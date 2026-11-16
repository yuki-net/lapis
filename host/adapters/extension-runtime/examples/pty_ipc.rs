//! smoke 用の模擬 Host。OS の PTY は起動しない。
use extension_runtime::supervisor::{
    ComponentImage, ExtensionSupervisor, HostCallHandler, SupervisorStatus,
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

struct Handler {
    denied: bool,
    calls: AtomicUsize,
}
impl HostCallHandler for Handler {
    fn call(
        &self,
        extension: &str,
        method: &str,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        if extension != "dev.example.pty" {
            return Err("wrong owner".into());
        }
        self.calls.fetch_add(1, Ordering::Relaxed);
        if self.denied {
            return Err("not approved".into());
        }
        match method {
            "pty.v1.spawn" if payload.get("program").is_some() => Ok(serde_json::json!("pty-1")),
            "pty.v1.attach" if payload == serde_json::json!({"id":"pty-1"}) => {
                Ok(serde_json::Value::Null)
            }
            _ => Err("unexpected call".into()),
        }
    }
}

fn main() {
    let mut args = std::env::args_os().skip(1);
    let worker = PathBuf::from(args.next().expect("worker"));
    let component = std::fs::read(args.next().expect("component")).expect("component bytes");
    let denied = args.next().expect("mode") == "denied";
    let handler = Arc::new(Handler {
        denied,
        calls: AtomicUsize::new(0),
    });
    let supervisor = ExtensionSupervisor::start_with_handler(
        worker,
        vec![ComponentImage {
            id: "dev.example.pty".into(),
            bytes: component,
        }],
        Some(handler.clone()),
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match supervisor.status() {
            SupervisorStatus::Running { active, failed, .. } => {
                assert!(failed.is_empty(), "{failed:?}");
                assert_eq!(active, ["dev.example.pty"]);
                assert_eq!(
                    handler.calls.load(Ordering::Relaxed),
                    if denied { 1 } else { 2 }
                );
                break;
            }
            SupervisorStatus::Stopped { reason } => panic!("{reason}"),
            SupervisorStatus::Starting => assert!(Instant::now() < deadline, "timeout"),
        }
        thread::sleep(Duration::from_millis(20));
    }
    drop(supervisor);
    assert_eq!(
        handler.calls.load(Ordering::Relaxed),
        if denied { 2 } else { 3 }
    );
}
