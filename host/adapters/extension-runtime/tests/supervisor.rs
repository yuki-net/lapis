use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use extension_runtime::supervisor::{ComponentImage, ExtensionSupervisor, SupervisorStatus};

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
            },
            ComponentImage {
                id: "test.ok".into(),
                bytes: component(),
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
