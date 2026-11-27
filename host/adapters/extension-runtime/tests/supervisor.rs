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
    assert_eq!(capabilities.process_info_version.as_deref(), Some("1.0.0"));
    assert_eq!(capabilities.dispatch_version.as_deref(), Some("1.0.0"));
    let legacy: extension_runtime::Response =
        serde_json::from_str(r#"{"ok":true,"lifecycle_version":"1.0.0"}"#).unwrap();
    assert!(legacy.pty_version.is_none());
    assert!(legacy.process_info_version.is_none());
    assert!(legacy.dispatch_version.is_none());
}

#[cfg(windows)]
#[test]
#[ignore = "cargo build --locked --manifest-path examples/native-extension/Cargo.toml --target-dir target/native-extension が必要"]
fn native_image_runs_in_worker_and_preserves_invocation_context() {
    use extension_runtime::{InvocationContext, supervisor::HostCallHandler};
    use std::sync::{Arc, Mutex};
    struct Handler(Mutex<Vec<(String, String)>>);
    impl HostCallHandler for Handler {
        fn call(
            &self,
            _: &str,
            _: &str,
            _: serde_json::Value,
        ) -> Result<serde_json::Value, String> {
            Err("context required".into())
        }
        fn call_in_context(
            &self,
            extension: &str,
            method: &str,
            payload: serde_json::Value,
            context: Option<&InvocationContext>,
        ) -> Result<serde_json::Value, String> {
            assert_eq!(extension, "lapis.native-example");
            assert_eq!(method, "echo.v1.echo");
            let context = context.ok_or("context required")?;
            self.0
                .lock()
                .unwrap()
                .push((context.session.clone(), context.workspace.clone()));
            Ok(payload)
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let bytes =
        std::fs::read(root.join("target/native-extension/debug/lapis_native_example.dll")).unwrap();
    let handler = Arc::new(Handler(Mutex::new(Vec::new())));
    // ローカルでビルドしたテスト用 image を使う。
    let supervisor = unsafe {
        ExtensionSupervisor::start_with_verified_native(
            PathBuf::from(env!("CARGO_BIN_EXE_lapis-extension-host")),
            vec![],
            vec![ComponentImage {
                id: "lapis.native-example".into(),
                bytes,
                require_dispatch: true,
            }],
            Some(handler.clone()),
        )
    };
    let status = wait_for(&supervisor, |status| {
        matches!(status, SupervisorStatus::Running { .. })
    });
    let SupervisorStatus::Running { active, failed, .. } = status else {
        unreachable!()
    };
    assert!(failed.is_empty(), "{failed:?}");
    assert_eq!(active, ["lapis.native-example"]);
    for workspace in ["first", "second"] {
        let context = InvocationContext {
            session: "session".into(),
            workspace: workspace.into(),
        };
        let result = supervisor
            .dispatch_with_context(
                "lapis.native-example",
                "echo.v1.echo",
                &format!(r#"{{"value":"{workspace}"}}"#),
                &context,
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&result).unwrap()["value"],
            workspace
        );
    }
    assert_eq!(
        *handler.0.lock().unwrap(),
        [
            ("session".into(), "first".into()),
            ("session".into(), "second".into())
        ]
    );
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
            (global $heap (mut i32) (i32.const 128))
            (data (i32.const 0) "\00\00\00\00\10\00\00\00\02\00\00\00")
            (func (export "activate"))
            (func (export "deactivate"))
            (func (export "realloc") (param i32 i32 i32 i32) (result i32)
              (local $address i32)
              (local.set $address (global.get $heap))
              (global.set $heap (i32.add (global.get $heap) (local.get 3)))
              (local.get $address))
            (func (export "dispatch") (param i32 i32 i32 i32 i32 i32 i32 i32) (result i32)
              (i32.store (i32.const 4) (local.get 4))
              (i32.store (i32.const 8) (local.get 5))
              (i32.const 0)))
          (core instance $i (instantiate $m))
          (func (export "activate") (canon lift (core func $i "activate")))
          (func (export "deactivate") (canon lift (core func $i "deactivate")))
          (func (export "dispatch")
            (param "operation" string) (param "payload" string)
            (param "session" string) (param "workspace" string)
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
            .dispatch_host_internal("test.dispatch", "inspect", "{}")
            .unwrap(),
        "host-internal"
    );
    let context = extension_runtime::InvocationContext {
        session: "session-123".into(),
        workspace: "workspace-456".into(),
    };
    assert_eq!(
        supervisor
            .dispatch_with_context("test.dispatch", "inspect", "{}", &context)
            .unwrap(),
        "session-123"
    );
    assert!(
        supervisor
            .dispatch_host_internal("test.legacy", "inspect", "{}")
            .is_err()
    );
    assert!(
        supervisor
            .dispatch_host_internal("test.missing", "inspect", "{}")
            .is_err()
    );
    assert!(
        supervisor
            .dispatch_host_internal("test.dispatch", "", "{}")
            .is_err()
    );
    assert!(
        supervisor
            .dispatch_host_internal(
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
fn dropping_a_running_supervisor_is_bounded() {
    let supervisor = ExtensionSupervisor::start(
        PathBuf::from(env!("CARGO_BIN_EXE_lapis-extension-host")),
        vec![ComponentImage {
            id: "test.worker".into(),
            bytes: component(),
            require_dispatch: false,
        }],
    );
    wait_for(&supervisor, |status| {
        matches!(status, SupervisorStatus::Running { .. })
    });
    let started = Instant::now();
    drop(supervisor);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "supervisor shutdown exceeded its bounded cleanup window"
    );
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
