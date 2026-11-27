"""Rust Core route: backend 全体の architecture / format / test / lint と外部拡張。"""

import sys

from scripts.ci.model import PYTHON_VERSION, Route, Step

RUSTFMT = "rustfmt"
CLIPPY = "clippy"


def py(*args):
    return [sys.executable, *args]


# Rust Core の外部拡張 component 検証。
EXTENSION_STEPS = (
    Step("Build lapis-terminal-native", ["cargo", "build", "--locked", "-p", "lapis-terminal-native"]),
    Step("Smoke native terminal", py("-m", "scripts.extensions.smoke_native_terminal")),
    Step("Test extension-runtime native", ["cargo", "test", "--locked", "-p", "extension-runtime", "--lib", "native::tests"]),
    Step(
        "Test native extension host",
        ["cargo", "test", "--locked", "-p", "extension-runtime", "--test", "native", "--", "--ignored"],
    ),
    Step(
        "Build external extension example",
        ["cargo", "build", "--locked", "--manifest-path", "examples/external-extension/Cargo.toml",
         "--target", "wasm32-wasip2", "--target-dir", "target/external-extension"],
    ),
    Step(
        "Build extension host",
        ["cargo", "build", "--locked", "-p", "extension-runtime", "--bin", "lapis-extension-host"],
    ),
    Step("Smoke extension host", py("-m", "scripts.extensions.smoke")),
    Step("Smoke pty extension", py("-m", "scripts.extensions.smoke_pty")),
    Step(
        "Stage bundled extensions",
        py("-m", "scripts.extensions.stage", "extensions", "{runner_temp}/lapis-bundled-input",
           "--debug"),
    ),
    Step(
        "Test independent component packaging",
        ["cargo", "test", "--locked", "-p", "extensions-entry", "--test", "package",
         "independent_component_is_packaged_installed_and_activated", "--", "--ignored"],
    ),
)

# Host 配下のうち、Host 専用の末端ではない領域。Host route より先に
# 評価されるので、ここに載せた path は rust_core が主になる。
HOST_SHARED_PATHS = (
    "host/extensions/",
    "host/extensions-entry/",
    "host/runtime/",
    "host/adapters/",
    "host/transport/",
    "host/application/",
)

ROUTE = Route(
    key="rust_core",
    name="Rust Core",
    runs_on=("linux", "windows"),
    timeout_minutes=40,
    python=PYTHON_VERSION,
    rust=True,
    rust_components=f"{RUSTFMT}, {CLIPPY}, rust-analyzer, rust-src",
    rust_targets="wasm32-wasip2",
    cache=True,
    steps=(
        Step("Check architecture", py("scripts/check.py", "rust_core", "architecture")),
        Step("Test Rust Core", py("scripts/check.py", "rust_core", "test")),
        *EXTENSION_STEPS,
    ),
)
