"""Desktop route: GPUI クライアントの検証と patch の確認。"""

import sys

from scripts.ci.model import PYTHON_VERSION, Route, Step


def py(*args):
    return [sys.executable, *args]


ROUTE = Route(
    key="desktop",
    name="Desktop",
    runs_on=("windows",),
    # crate 移設で Cargo の path fingerprint が無効になり、cold build に
    # なることがある。テスト本体は 40 分（cold で 20 分を超えた）、
    # 事前ビルドは別枠で余裕を持たせる。
    timeout_minutes=90,
    python=PYTHON_VERSION,
    rust=True,
    rust_components="rustfmt, clippy",
    # terminal component の wasm をビルドするため。
    rust_targets="wasm32-wasip2",
    cache=True,
    steps=(
        Step("Check architecture", py("scripts/check.py", "desktop", "architecture")),
        Step("Check formatting", py("scripts/check.py", "desktop", "format")),
        # テストの前提になるビルド。3 step で合計 35 分を上限の目安にする。
        Step(
            "Build extension host",
            ["cargo", "build", "--locked", "-p", "extension-runtime", "--bin", "lapis-extension-host"],
            timeout_minutes=12,
        ),
        Step(
            "Build lapis-terminal-native",
            ["cargo", "build", "--locked", "-p", "lapis-terminal-native"],
            timeout_minutes=12,
        ),
        # terminal component の wasm は、拡張 package を作るテストの前提。
        Step(
            "Build terminal component",
            ["cargo", "build", "--locked", "--manifest-path", "extensions/terminal/component/Cargo.toml",
             "--target", "wasm32-wasip2", "--target-dir", "target/terminal-component"],
            timeout_minutes=12,
        ),
        Step("Test Desktop", py("scripts/check.py", "desktop", "test"), timeout_minutes=40),
        # lapis（Desktop 全体）をビルドする必要があるテスト。Desktop の
        # ビルド済み環境でだけ実行する。Host ジョブでは lapis のビルドが
        # job の上限を超える。
        Step(
            "Test desktop terminal pty",
            ["cargo", "test", "--locked", "-p", "lapis", "desktop_terminal_runs_real_extension_pty", "--", "--ignored"],
        ),
        Step("Lint Desktop", py("scripts/check.py", "desktop", "lint")),
        Step("Build Desktop", py("scripts/check.py", "desktop", "build")),
        Step("Verify GPUI patches", py("-m", "scripts.ci.patches")),
    ),
)
