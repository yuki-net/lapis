"""Host route: Windows の Host 検証（承認済み拡張 PTY・Web 公開を目視検証を含む）。"""

import sys

from scripts.ci.model import NODE_VERSION, PYTHON_VERSION, WASM_BINDGEN_VERSION, Route, Step


def py(*args):
    return [sys.executable, *args]


# 承認済み拡張 PTY 検証。以前は 1 step に `$LASTEXITCODE` 付きで並べていたが、
# argv の step に分けて失敗箇所を一意にする。
PTY_STEPS = (
    Step(
        "Build native extension fixture",
        ["cargo", "build", "--locked", "--manifest-path", "examples/native-extension/Cargo.toml",
         "--target-dir", "target/native-extension"],
    ),
    Step("Test extension-runtime native", ["cargo", "test", "--locked", "-p", "extension-runtime", "--lib", "native::tests"]),
    Step(
        "Test staged image is locked",
        ["cargo", "test", "--locked", "-p", "extension-runtime", "--lib",
         "staged_image_is_locked_until_extension_is_unloaded", "--", "--ignored"],
    ),
    Step(
        "Test supervisor keeps invocation context",
        ["cargo", "test", "--locked", "-p", "extension-runtime", "--test", "supervisor",
         "native_image_runs_in_worker_and_preserves_invocation_context", "--", "--ignored"],
    ),
    Step(
        "Test native extension host",
        ["cargo", "test", "--locked", "-p", "extension-runtime", "--test", "native", "--", "--ignored"],
    ),
    Step("Test local-process pty", ["cargo", "test", "--locked", "-p", "local-process", "pty::tests"]),
    Step(
        "Build extension host",
        ["cargo", "build", "--locked", "-p", "extension-runtime", "--bin", "lapis-extension-host"],
    ),
    Step(
        "Test signed native package in host",
        ["cargo", "test", "--locked", "-p", "host", "signed_native_package_runs_in_extension_host", "--", "--ignored"],
    ),
    Step(
        "Build pty example extension",
        ["cargo", "build", "--locked", "--manifest-path", "examples/pty-extension/Cargo.toml",
         "--target", "wasm32-wasip2", "--target-dir", "target/pty-extension"],
    ),
    Step(
        "Test approved wasm pty",
        ["cargo", "test", "--locked", "-p", "host", "approved_wasm_uses_host_pty_and_shutdown_reclaims_it", "--", "--ignored"],
    ),
)

ROUTE = Route(
    key="host",
    name="Host",
    runs_on=("windows",),
    # 検証だけで約 44 分かかる（承認済み拡張 PTY の 10 step を含む）。
    # さらに job の後処理で target/ のキャッシュ保存に数分かかるため、
    # 旧 CI の 50 分では足りずに保存中に打ち切られる。上限は保存分を含めて決める。
    timeout_minutes=75,
    python=PYTHON_VERSION,
    node=NODE_VERSION,
    rust=True,
    rust_components="rustfmt, clippy",
    rust_targets="wasm32-wasip2, wasm32-unknown-unknown",
    wasm_bindgen=WASM_BINDGEN_VERSION,
    cache=True,
    steps=(
        Step("Check architecture", py("scripts/check.py", "host", "architecture")),
        Step("Test Host", py("scripts/check.py", "host", "test"), timeout_minutes=20),
        *PTY_STEPS,
        Step("Build host and web bins", ["cargo", "build", "--locked", "-p", "host", "-p", "lapis-web", "--bins"]),
        Step("Build web page server", ["cargo", "build", "--locked", "-p", "lapis-web"]),
        Step(
            "Test managed Web publication",
            ["cargo", "test", "--locked", "-p", "host", "--test", "managed_web_auth",
             "managed_host_owns_web_publication_and_stops_it_on_exit", "--", "--ignored"],
            env={"LAPIS_TEST_WEB_EXECUTABLE": "{root}/target/debug/lapis-web{exe}"},
        ),
        Step("Build Host", py("scripts/check.py", "host", "build")),
    ),
)
