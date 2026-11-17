use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use extension_runtime::supervisor::{ComponentImage, ExtensionSupervisor, SupervisorStatus};

#[test]
fn worker_reports_each_supported_contract_version() {
    let worker = PathBuf::from(env!("CARGO_BIN_EXE_lapis-extension-host"));
    let capabilities = extension_runtime::supervisor::probe_worker(&worker).unwrap();
    assert_eq!(capabilities.lifecycle_version.as_deref(), Some("1.0.0"));
    assert_eq!(capabilities.pty_version.as_deref(), Some("1.0.0"));
    assert_eq!(capabilities.dispatch_version.as_deref(), Some("1.0.0"));
    let legacy: extension_runtime::Response =
        serde_json::from_str(r#"{"ok":true,"lifecycle_version":"1.0.0"}"#).unwrap();
    assert!(legacy.pty_version.is_none());
    assert!(legacy.dispatch_version.is_none());
}

fn component() -> Vec<u8> {
    wat::parse_str(
        r#"
        (component
          (core module $m (func (export "activate")) (func (export "deactivate")))
          (core instance $i (instantiate $m))
          (func (export "activate") (canon lift (core func $i "activate")))
          (func (export "deactivate") (canon lift (core func $i "deactivate"))))
    "#,
    )
    .unwrap()
}

fn dispatch_component() -> Vec<u8> {
    wat::parse_str(
        r#"
        (component
          (core module $m
            (memory (export "memory") 1)
            (data (i32.const 0) "\00\00\00\00\10\00\00\00\02\00\00\00")
            (func (export "activate"))
            (func (export "deactivate"))
            (func (export "realloc") (param i32 i32 i32 i32) (result i32)
              (i32.const 128))
            (func (export "dispatch") (param i32 i32 i32 i32) (result i32)
              (i32.store (i32.const 4) (local.get 2))
              (i32.store (i32.const 8) (local.get 3))
              (i32.const 0)))
          (core instance $i (instantiate $m))
          (func (export "activate") (canon lift (core func $i "activate")))
          (func (export "deactivate") (canon lift (core func $i "deactivate")))
          (func (export "dispatch")
            (param "operation" string) (param "payload" string)
            (result (result string (error string)))
            (canon lift (core func $i "dispatch")
              (memory (core memory $i "memory")) (realloc (core func $i "realloc")))))
        "#,
    )
    .unwrap()
}

#[test]
fn dispatch_reaches_only_the_requested_active_component() {
    let supervisor = ExtensionSupervisor::start(
        PathBuf::from(env!("CARGO_BIN_EXE_lapis-extension-host")),
        vec![
            ComponentImage {
                id: "test.dispatch".into(),
                bytes: dispatch_component(),
                require_dispatch: true,
            },
            ComponentImage {
                id: "test.legacy".into(),
                bytes: component(),
                require_dispatch: false,
            },
        ],
    );
    wait_for(&supervisor, |status| {
        matches!(status, SupervisorStatus::Running { .. })
    });
    assert_eq!(
        supervisor
            .dispatch("test.dispatch", "inspect", "{}")
            .unwrap(),
        "{}"
    );
    assert!(supervisor.dispatch("test.legacy", "inspect", "{}").is_err());
    assert!(
        supervisor
            .dispatch("test.missing", "inspect", "{}")
            .is_err()
    );
    assert!(supervisor.dispatch("test.dispatch", "", "{}").is_err());
    assert!(
        supervisor
            .dispatch(
                "test.dispatch",
                "inspect",
                &"x".repeat(extension_runtime::MAX_DISPATCH_BYTES + 1)
            )
            .is_err()
    );
}

fn wait_for(
    supervisor: &ExtensionSupervisor,
    condition: impl Fn(&SupervisorStatus) -> bool,
) -> SupervisorStatus {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let status = supervisor.status();
        if condition(&status) {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "status did not change: {status:?}"
        );
        thread::sleep(Duration::from_millis(30));
    }
}

#[test]
fn activation_failures_do_not_prevent_other_extensions() {
    let supervisor = ExtensionSupervisor::start(
        PathBuf::from(env!("CARGO_BIN_EXE_lapis-extension-host")),
        vec![
            ComponentImage {
                id: "test.invalid".into(),
                bytes: vec![0],
                require_dispatch: false,
            },
            ComponentImage {
                id: "test.ok".into(),
                bytes: component(),
                require_dispatch: false,
            },
            ComponentImage {
                id: "test.missing-dispatch".into(),
                bytes: component(),
                require_dispatch: true,
            },
        ],
    );
    let result = wait_for(&supervisor, |status| {
        matches!(status, SupervisorStatus::Running { .. })
    });
    let SupervisorStatus::Running { active, failed, .. } = result else {
        unreachable!()
    };
    assert_eq!(active, vec!["test.ok"]);
    assert!(failed.contains_key("test.invalid"));
    assert!(failed.contains_key("test.missing-dispatch"));
}

#[test]
fn missing_executable_is_reported_after_retry() {
    let supervisor =
        ExtensionSupervisor::start(PathBuf::from("this-worker-does-not-exist"), vec![]);
    let result = wait_for(&supervisor, |status| {
        matches!(status, SupervisorStatus::Stopped { .. })
    });
    assert!(matches!(result, SupervisorStatus::Stopped { reason } if reason.contains("起動")));
}

#[test]
fn crashed_worker_is_restarted_once_with_the_original_snapshot() {
    let supervisor = ExtensionSupervisor::start(
        PathBuf::from(env!("CARGO_BIN_EXE_lapis-extension-host")),
        vec![ComponentImage {
            id: "test.worker".into(),
            bytes: component(),
            require_dispatch: false,
        }],
    );
    let running = wait_for(&supervisor, |status| {
        matches!(status, SupervisorStatus::Running { .. })
    });
    let SupervisorStatus::Running {
        pid: first, active, ..
    } = running
    else {
        unreachable!()
    };
    assert_eq!(active, vec!["test.worker"]);
    kill_worker(first);
    let running = wait_for(
        &supervisor,
        |status| matches!(status, SupervisorStatus::Running { pid, .. } if *pid != first),
    );
    let SupervisorStatus::Running {
        pid: second,
        active,
        ..
    } = running
    else {
        unreachable!()
    };
    assert_eq!(active, vec!["test.worker"]);
    kill_worker(second);
    wait_for(&supervisor, |status| {
        matches!(status, SupervisorStatus::Stopped { .. })
    });
}

fn kill_worker(pid: u32) {
    #[cfg(windows)]
    let mut command = {
        use std::os::windows::process::CommandExt;
        let mut command = std::process::Command::new("taskkill");
        command
            .creation_flags(0x08000000)
            .args(["/F", "/PID", &pid.to_string()]);
        command
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut command = std::process::Command::new("kill");
        command.args(["-9", &pid.to_string()]);
        command
    };
    assert!(
        command
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    );
}
