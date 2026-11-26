#![cfg(feature = "worker")]

use std::sync::{Arc, Mutex};

use extension_runtime::{ExtensionRuntime, InvocationContext, Request, native::NativeExtension};
use serde_json::Value;

fn fixture_path() -> std::path::PathBuf {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let name = if cfg!(windows) {
        "lapis_native_example.dll"
    } else if cfg!(target_os = "macos") {
        "liblapis_native_example.dylib"
    } else {
        "liblapis_native_example.so"
    };
    root.join("target/native-extension/debug")
        .join(name)
        .canonicalize()
        .unwrap()
}

/// 実際の共有ライブラリを使うため、先に native fixture をビルドする。
#[test]
#[ignore = "cargo build --locked --manifest-path examples/native-extension/Cargo.toml --target-dir target/native-extension が必要"]
fn native_extension_uses_generic_host_broker_and_lifecycle() {
    let path = fixture_path();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let observed = calls.clone();
    let contexts = Arc::new(Mutex::new(Vec::new()));
    let observed_contexts = contexts.clone();
    let broker = Arc::new(
        move |method: &str, payload: Value, context: Option<&InvocationContext>| {
            observed.lock().unwrap().push(method.to_owned());
            observed_contexts
                .lock()
                .unwrap()
                .push(context.map(|context| (context.session.clone(), context.workspace.clone())));
            match method {
                "echo.v1.echo" => {
                    assert_eq!(payload["value"], "hello");
                    Ok(payload)
                }
                _ => Err("unknown Host capability".into()),
            }
        },
    );
    // この fixture はテスト直前にローカルでビルドしたファイルだけを読み込む。
    let mut extension = unsafe { NativeExtension::load_verified(&path, broker) }.unwrap();
    let context = InvocationContext {
        session: "session".into(),
        workspace: "workspace".into(),
    };
    let echoed: Value = serde_json::from_str(
        &extension
            .dispatch("echo.v1.echo", r#"{"value":"hello"}"#, &context)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(echoed["value"], "hello");
    extension.deactivate().unwrap();
    assert!(
        extension
            .dispatch("echo.v1.echo", r#"{"value":"hello"}"#, &context)
            .is_err()
    );
    assert_eq!(&*calls.lock().unwrap(), &["echo.v1.echo"]);
    assert_eq!(
        &*contexts.lock().unwrap(),
        &[Some(("session".into(), "workspace".into()))]
    );
}

#[test]
#[ignore = "cargo build --locked --manifest-path examples/native-extension/Cargo.toml --target-dir target/native-extension が必要"]
fn native_and_wasm_share_the_runtime_lifecycle() {
    let path = fixture_path();
    let broker = Arc::new(
        |method: &str, payload: Value, context: Option<&InvocationContext>| {
            assert_eq!(context.unwrap().workspace, "workspace");
            match method {
                "echo.v1.echo" => Ok(payload),
                _ => Err("unknown Host capability".into()),
            }
        },
    );
    let bytes = wat::parse_str(
        r#"
        (component
          (core module $m (func (export "activate")) (func (export "deactivate")))
          (core instance $i (instantiate $m))
          (func (export "activate") (canon lift (core func $i "activate")))
          (func (export "deactivate") (canon lift (core func $i "deactivate"))))
    "#,
    )
    .unwrap();
    let mut runtime = ExtensionRuntime::new().unwrap();
    runtime.activate("example.wasm", &bytes).unwrap();
    // ローカルで事前にビルドした fixture だけを読み込む。
    unsafe { runtime.activate_native_verified("example.native", &path, broker.clone()) }.unwrap();
    assert!(runtime.activate("example.native", &bytes).is_err());
    assert!(unsafe { runtime.activate_native_verified("example.wasm", &path, broker) }.is_err());
    let context = InvocationContext {
        session: "session".into(),
        workspace: "workspace".into(),
    };
    let response: Value = serde_json::from_str(
        &runtime
            .dispatch(
                "example.native",
                "echo.v1.echo",
                r#"{"value":"shared"}"#,
                &context,
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(response["value"], "shared");
    assert!(runtime.handle(Request::Shutdown).ok);
    assert!(
        runtime
            .dispatch(
                "example.native",
                "echo.v1.echo",
                r#"{"value":"shared"}"#,
                &context
            )
            .is_err()
    );
    assert!(runtime.deactivate("example.wasm").is_err());
}

#[cfg(windows)]
#[test]
#[ignore = "cargo build --locked --manifest-path examples/native-extension/Cargo.toml --target-dir target/native-extension が必要"]
fn verified_native_bytes_can_be_loaded_without_reopening_package_path() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../target/native-extension/debug/lapis_native_example.dll");
    let bytes = std::fs::read(path).unwrap();
    let broker = Arc::new(
        |method: &str, payload: Value, _: Option<&InvocationContext>| match method {
            "echo.v1.echo" => Ok(payload),
            _ => Err("unknown Host capability".into()),
        },
    );
    // この fixture はテスト用にローカルでビルドした実行コード。
    let mut extension = unsafe { NativeExtension::load_verified_bytes(&bytes, broker) }.unwrap();
    let context = InvocationContext {
        session: "session".into(),
        workspace: "workspace".into(),
    };
    let value: Value = serde_json::from_str(
        &extension
            .dispatch("echo.v1.echo", r#"{"value":"bytes"}"#, &context)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(value["value"], "bytes");
}
