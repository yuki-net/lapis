use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use extension_runtime::{ExtensionRuntime, Request, Response};

fn component(activate_body: &str, deactivate_body: &str) -> Vec<u8> {
    wat::parse_str(format!(
        r#"
        (component
          (core module $module
            (global $state (mut i32) (i32.const 0))
            (func (export "activate") {activate_body})
            (func (export "deactivate") {deactivate_body}))
          (core instance $instance (instantiate $module))
          (func (export "activate") (canon lift (core func $instance "activate")))
          (func (export "deactivate") (canon lift (core func $instance "deactivate"))))
    "#
    ))
    .unwrap()
}

#[test]
fn lifecycle_isolated_per_extension() {
    let mut runtime = ExtensionRuntime::new().unwrap();
    let bytes = component(
        "i32.const 1 global.set $state",
        "global.get $state i32.eqz if unreachable end",
    );
    runtime.activate("example.first", &bytes).unwrap();
    runtime.activate("example.second", &bytes).unwrap();
    assert!(runtime.activate("example.first", &bytes).is_err());
    runtime.deactivate("example.first").unwrap();
    assert!(runtime.deactivate("example.first").is_err());
    runtime.deactivate("example.second").unwrap();
}

#[test]
fn trapped_or_nonterminating_component_cannot_stay_active() {
    let mut runtime = ExtensionRuntime::new().unwrap();
    assert!(
        runtime
            .activate("example.loop", &component("loop $l br $l end", ""))
            .is_err()
    );
    assert!(runtime.deactivate("example.loop").is_err());
    runtime
        .activate("example.trap", &component("", "unreachable"))
        .unwrap();
    assert!(runtime.deactivate("example.trap").is_err());
    assert!(runtime.deactivate("example.trap").is_err());
    runtime
        .activate("example.next", &component("", ""))
        .unwrap();
}

#[test]
fn component_with_memory_over_limit_is_not_activated() {
    let bytes = wat::parse_str(
        r#"
        (component
          (core module $module
            (memory 1025)
            (func (export "activate"))
            (func (export "deactivate")))
          (core instance $instance (instantiate $module))
          (func (export "activate") (canon lift (core func $instance "activate")))
          (func (export "deactivate") (canon lift (core func $instance "deactivate"))))
    "#,
    )
    .unwrap();
    let mut runtime = ExtensionRuntime::new().unwrap();
    assert!(runtime.activate("example.memory", &bytes).is_err());
    assert!(runtime.deactivate("example.memory").is_err());
}

#[test]
fn components_must_implement_both_lifecycle_functions() {
    let bytes = wat::parse_str(
        r#"
        (component
          (core module $module (func (export "activate")))
          (core instance $instance (instantiate $module))
          (func (export "activate") (canon lift (core func $instance "activate"))))
    "#,
    )
    .unwrap();
    let mut runtime = ExtensionRuntime::new().unwrap();
    assert!(runtime.activate("example.incomplete", &bytes).is_err());
    assert!(runtime.deactivate("example.incomplete").is_err());
}

#[test]
fn worker_accepts_serialized_component_in_a_separate_process() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lapis-extension-host"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let bytes = component("", "");
    for request in [
        Request::Activate {
            id: "example.worker".into(),
            component: STANDARD.encode(bytes),
        },
        Request::Deactivate {
            id: "example.worker".into(),
        },
        Request::Shutdown,
    ] {
        serde_json::to_writer(&mut input, &request).unwrap();
        input.write_all(b"\n").unwrap();
        input.flush().unwrap();
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        let response: Response = serde_json::from_str(&line).unwrap();
        assert!(response.ok, "{:?}", response.error);
    }
    assert!(child.wait().unwrap().success());
}
