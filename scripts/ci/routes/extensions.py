"""Extensions route: Windows の拡張 native / PTY 検証。"""

import sys

from scripts.ci.model import PYTHON_VERSION, Route, Step


def py(*args):
    return [sys.executable, *args]


# 承認済み拡張の native 実行と PTY。以前は Host route に置いていたが、
# Host の build 結果ではなく拡張側の検証なので独立した route で走らせる。
# 実行順は fixture のビルドを先頭に、検証対象の依存順で並べる。
STEPS = (
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
    key="extensions",
    name="Extensions",
    # 承認済み拡張の native 実行は Windows の Host バイナリと PTY を前提にする。
    runs_on=("windows",),
    # Host から分離した 10 step（fixture のビルドと native / PTY 検証）。
    # job の後処理で target/ のキャッシュ保存に数分かかるため、上限は
    # 保存分を含めて決める。
    timeout_minutes=45,
    python=PYTHON_VERSION,
    rust=True,
    rust_components="rustfmt, clippy",
    rust_targets="wasm32-wasip2",
    cache=True,
    steps=STEPS,
)
