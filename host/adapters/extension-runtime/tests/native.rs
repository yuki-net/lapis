#![cfg(feature = "worker")]

use std::sync::{Arc, Mutex};

use extension_runtime::{InvocationContext, native::NativeExtension};
use serde_json::{Value, json};

/// 実際の共有ライブラリを使うため、先に native Terminal をビルドする。
#[test]
#[ignore = "cargo build --locked -p lapis-terminal-native が必要"]
fn native_terminal_uses_generic_host_broker_and_lifecycle() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let name = if cfg!(windows) {
        "lapis_terminal_native.dll"
    } else if cfg!(target_os = "macos") {
        "liblapis_terminal_native.dylib"
    } else {
        "liblapis_terminal_native.so"
    };
    let path = root.join("target/debug").join(name).canonicalize().unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let observed = calls.clone();
    let broker = Arc::new(move |method: &str, payload: Value| {
        observed.lock().unwrap().push(method.to_owned());
        match method {
            "process.info.v1.get" => {
                Ok(json!({"pty":"conpty","default_shell":{"program":"shell","arguments":[]}}))
            }
            "pty.v1.spawn" => {
                assert_eq!(payload["cwd"], ".");
                Ok(json!("pty-1"))
            }
            "pty.v1.attach" | "pty.v1.detach" | "pty.v1.kill" | "pty.v1.forget" => Ok(Value::Null),
            "pty.v1.read" => Ok(
                json!({"bytes":[111,107],"next":2,"lost_before":null,"exited":null,"read_error":null,"write_error":null}),
            ),
            _ => Err("unknown Host capability".into()),
        }
    });
    // この fixture はテスト直前にローカルでビルドしたファイルだけを読み込む。
    let mut extension = unsafe { NativeExtension::load_verified(&path, broker) }.unwrap();
    let context = InvocationContext {
        session: "session".into(),
        workspace: "workspace".into(),
    };
    let started: Value = serde_json::from_str(
        &extension
            .dispatch(
                "terminal.v1.start-profile",
                r#"{"profile":null,"working_directory":".","columns":80,"rows":24}"#,
                &context,
            )
            .unwrap(),
    )
    .unwrap();
    let read = json!({"id":started["id"],"offset":0,"max_bytes":8,"timeout_ms":0}).to_string();
    let output: Value = serde_json::from_str(
        &extension
            .dispatch("terminal.v1.read", &read, &context)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(output["bytes"], json!([111, 107]));
    let other = InvocationContext {
        session: "session".into(),
        workspace: "other".into(),
    };
    assert!(
        extension
            .dispatch("terminal.v1.read", &read, &other)
            .is_err()
    );
    extension.deactivate().unwrap();
    assert_eq!(
        &*calls.lock().unwrap(),
        &[
            "process.info.v1.get",
            "pty.v1.spawn",
            "pty.v1.attach",
            "pty.v1.read",
            "pty.v1.kill",
            "pty.v1.detach"
        ]
    );
}
