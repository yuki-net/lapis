"""CI の job 定義。YAML ではなくここを唯一の正本とする。

`.github/workflows/ci.yml` は `plan` が返す matrix をそのまま展開するだけで、
実行する step 列はこのファイルが持つ。ローカルと GitHub は同じ step を
同じコマンドで実行し、前提（OS・toolchain）を満たさない route だけを
`unsupported` として明示する。

step は shell 文字列ではなく argv で持つ。Windows / Linux で
`&&` や `$LASTEXITCODE` に頼らず、失敗で即座に止まるようにするため。
"""

from dataclasses import dataclass, field
from pathlib import Path
import os
import sys

ROOT = Path(__file__).resolve().parents[2]

# CI の固定値。ローカルの検証でも同じ値を使う。
RUST_TOOLCHAIN = "1.96.0"
PYTHON_VERSION = "3.12"
NODE_VERSION = "24"
WASM_BINDGEN_VERSION = "0.2.129"
JAVA_VERSION = "21"

# GitHub Actions の runner。ローカルでは os の一致だけを見る。
# os="any" は OS に依存しない検証（python の静的検査など）を表す。
RUNNERS = {"linux": "ubuntu-latest", "windows": "windows-2022", "any": "ubuntu-latest"}


def py(*args):
    """現在の Python で script を実行する argv を作る。"""
    return [sys.executable, *args]


@dataclass(frozen=True)
class Step:
    """1 つの検証。name は CI の折りたたみ表示とローカルの表に使う。"""

    name: str
    command: list[str]
    # route の作業ディレクトリからの相対パス。既定は route のディレクトリ。
    directory: str | None = None
    # 小数も許す（テストで極小の上限を渡すため）。
    timeout_minutes: float | None = None
    retries: int = 0
    retry_delay_seconds: int = 0
    # 前提を満たさない環境では実行せず unsupported として扱う。
    requires: tuple[str, ...] = ()
    # GitHub では `::error` 注釈を出さず成功扱いにする表示専用の step。
    informational: bool = False
    env: dict[str, str] = field(default_factory=dict)


@dataclass(frozen=True)
class Route:
    """path routing の 1 ルート（= 旧 ci.yml の 1 job）。"""

    key: str
    name: str
    os: str
    timeout_minutes: int
    steps: tuple[Step, ...]
    # checkout の fetch-depth。0 は全履歴。
    checkout_depth: int = 1
    python: str = ""
    node: str = ""
    node_cache: str = ""
    node_cache_path: str = ""
    rust_components: str = ""
    rust_targets: str = ""
    rust: bool = False
    wasm_bindgen: str = ""
    java: str = ""
    gradle: bool = False
    # actions/cache の key 接頭辞。空ならキャッシュしない。
    cache: str = ""
    # route の既定作業ディレクトリ（リポジトリルートからの相対パス）。
    working_directory: str = ""


RUSTFMT = "rustfmt"
CLIPPY = "clippy"

# Host の承認済み拡張 PTY 検証。以前は 1 step に `$LASTEXITCODE` 付きで
# 並べていたが、argv の step に分けて失敗箇所を一意にする。
_HOST_PTY_STEPS = (
    Step("Build lapis-terminal-native", ["cargo", "build", "--locked", "-p", "lapis-terminal-native"]),
    Step(
        "Stage native extension input",
        py("-m", "scripts.extensions.stage", "extensions", "{runner_temp}/lapis-native-input",
           "--target-dir", "target", "--debug"),
    ),
    Step("Smoke native terminal", py("-m", "scripts.extensions.smoke_native_terminal")),
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
        "Test native terminal survives QUIC disconnect",
        ["cargo", "test", "--locked", "-p", "host", "--test", "quic",
         "native_terminal_survives_quic_disconnect_and_can_be_claimed", "--", "--ignored"],
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
    Step(
        "Build terminal component",
        ["cargo", "build", "--locked", "--manifest-path", "extensions/terminal/component/Cargo.toml",
         "--target", "wasm32-wasip2", "--target-dir", "target/terminal-component"],
    ),
    Step(
        "Test terminal component pty",
        ["cargo", "test", "--locked", "-p", "host", "terminal_component_uses_approved_host_pty", "--", "--ignored"],
    ),
)

# Rust Core の外部拡張 component 検証。
_RUST_CORE_EXTENSION_STEPS = (
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
    Step("Smoke terminal extension", py("-m", "scripts.extensions.smoke_terminal")),
    Step(
        "Stage bundled extensions",
        py("-m", "scripts.extensions.stage", "extensions", "{runner_temp}/lapis-bundled-input",
           "--target-dir", "target/terminal-component", "--debug"),
    ),
    Step(
        "Check terminal component formatting",
        ["cargo", "fmt", "--manifest-path", "extensions/terminal/component/Cargo.toml", "--", "--check"],
    ),
    Step(
        "Lint terminal component",
        ["cargo", "clippy", "--locked", "--manifest-path", "extensions/terminal/component/Cargo.toml",
         "--target", "wasm32-wasip2", "--target-dir", "target/terminal-component", "--", "-D", "warnings"],
    ),
    Step(
        "Test independent component packaging",
        ["cargo", "test", "--locked", "-p", "extensions-entry", "--test", "package",
         "independent_component_is_packaged_installed_and_activated", "--", "--ignored"],
    ),
)

ROUTES: dict[str, Route] = {
    "repository": Route(
        key="repository",
        name="Repository",
        os="linux",
        timeout_minutes=20,
        checkout_depth=0,
        python=PYTHON_VERSION,
        steps=(
            Step("Run repository checks", py("scripts/check.py", "repository", "architecture")),
        ),
    ),
    "dev_windows": Route(
        key="dev_windows",
        name="Dev Commands / Windows",
        os="windows",
        timeout_minutes=20,
        python=PYTHON_VERSION,
        rust=True,
        rust_components=RUSTFMT,
        steps=(
            Step("Test dev commands", py("-m", "unittest", "discover", "-s", "scripts", "-p", "test_dev_commands.py")),
            Step("Check dev-session formatting", ["cargo", "fmt", "--manifest-path", "scripts/dev-session/Cargo.toml", "--", "--check"]),
            Step("Test dev session", py("-m", "unittest", "discover", "-s", "scripts", "-p", "test_dev_session.py")),
        ),
    ),
    "rust_core": Route(
        key="rust_core",
        name="Rust Core",
        os="linux",
        timeout_minutes=40,
        python=PYTHON_VERSION,
        rust=True,
        rust_components=f"{RUSTFMT}, {CLIPPY}, rust-analyzer, rust-src",
        rust_targets="wasm32-wasip2",
        cache="rust-core",
        steps=(
            Step("Check architecture", py("scripts/check.py", "rust_core", "architecture")),
            Step("Check formatting", py("scripts/check.py", "rust_core", "format")),
            Step("Test Rust Core", py("scripts/check.py", "rust_core", "test")),
            *_RUST_CORE_EXTENSION_STEPS,
            Step("Lint Rust Core", py("scripts/check.py", "rust_core", "lint")),
        ),
    ),
    "host": Route(
        key="host",
        name="Host / Windows",
        os="windows",
        timeout_minutes=50,
        python=PYTHON_VERSION,
        node=NODE_VERSION,
        rust=True,
        rust_components=f"{RUSTFMT}, {CLIPPY}",
        rust_targets="wasm32-wasip2, wasm32-unknown-unknown",
        wasm_bindgen=WASM_BINDGEN_VERSION,
        cache="host",
        steps=(
            Step("Check architecture", py("scripts/check.py", "host", "architecture")),
            Step("Check formatting", py("scripts/check.py", "host", "format")),
            Step("Test Host", py("scripts/check.py", "host", "test"), timeout_minutes=20),
            *_HOST_PTY_STEPS,
            Step("Build host and web-bridge bins", ["cargo", "build", "--locked", "-p", "host", "-p", "web-bridge", "--bins"]),
            Step(
                "Build web terminal smoke",
                ["cargo", "build", "--locked", "-p", "web-terminal-smoke", "--target", "wasm32-unknown-unknown",
                 "--profile", "web"],
            ),
            Step("Test web extension terminal", py("clients/web/wasm/tests/extension_terminal_smoke.py")),
            Step("Build web-bridge", ["cargo", "build", "--locked", "-p", "web-bridge"]),
            Step(
                "Test managed Web publication",
                ["cargo", "test", "--locked", "-p", "host", "--test", "managed_web_auth",
                 "managed_host_owns_web_publication_and_stops_it_on_exit", "--", "--ignored"],
                env={"LAPIS_TEST_WEB_BRIDGE": "{root}/target/debug/lapis-web-bridge{exe}"},
            ),
            Step("Lint Host", py("scripts/check.py", "host", "lint")),
            Step("Build Host", py("scripts/check.py", "host", "build")),
        ),
    ),
    "desktop": Route(
        key="desktop",
        name="Desktop / Windows",
        os="windows",
        timeout_minutes=60,
        python=PYTHON_VERSION,
        rust=True,
        rust_components=f"{RUSTFMT}, {CLIPPY}",
        cache="desktop",
        steps=(
            Step("Check architecture", py("scripts/check.py", "desktop", "architecture")),
            Step("Check formatting", py("scripts/check.py", "desktop", "format")),
            Step(
                "Build extension host",
                ["cargo", "build", "--locked", "-p", "extension-runtime", "--bin", "lapis-extension-host"],
            ),
            Step("Build lapis-terminal-native", ["cargo", "build", "--locked", "-p", "lapis-terminal-native"]),
            Step("Test Desktop", py("scripts/check.py", "desktop", "test"), timeout_minutes=20),
            # Desktop を丸ごとビルドする必要があるテストは、Desktop のビルド済み
            # 環境でだけ実行する。Host ジョブで走らせると lapis のビルドが
            # 間に合わず job の上限まで張り付く（旧 CI も Desktop 側で実行していた）。
            Step(
                "Test desktop terminal pty",
                ["cargo", "test", "--locked", "-p", "lapis", "desktop_terminal_runs_real_extension_pty", "--", "--ignored"],
            ),
            Step("Lint Desktop", py("scripts/check.py", "desktop", "lint")),
            Step("Build Desktop", py("scripts/check.py", "desktop", "build")),
            Step(
                "Verify GPUI patches",
                py("-m", "scripts.ci.patches"),
            ),
        ),
    ),
    "mobile": Route(
        key="mobile",
        name="Mobile / Android",
        os="linux",
        timeout_minutes=45,
        java=JAVA_VERSION,
        gradle=True,
        working_directory="clients/mobile",
        steps=(
            Step("Make Gradle wrapper executable", ["chmod", "+x", "./gradlew"], requires=("posix",)),
            Step(
                "Test shared KMP code",
                ["./gradlew", ":sharedUi:testDebugUnitTest", "--stacktrace"],
                retries=2,
                retry_delay_seconds=15,
            ),
            Step(
                "Build Android app",
                ["./gradlew", ":androidApp:assembleDebug", "--stacktrace"],
                retries=2,
                retry_delay_seconds=15,
            ),
        ),
    ),
    "web": Route(
        key="web",
        name="Web",
        os="linux",
        timeout_minutes=20,
        node=NODE_VERSION,
        node_cache="npm",
        node_cache_path="clients/web/package-lock.json",
        rust=True,
        rust_targets="wasm32-unknown-unknown",
        wasm_bindgen=WASM_BINDGEN_VERSION,
        working_directory="clients/web",
        steps=(
            Step("Install dependencies", ["npm", "ci"]),
            Step("Build Web", ["npm", "run", "build"]),
            Step("Test Web connection handling", ["npm", "test"]),
        ),
    ),
}

# 実行順。matrix の並び順もここに従う。
ORDER = ("repository", "dev_windows", "rust_core", "host", "desktop", "mobile", "web")


def route_for(key: str) -> Route:
    try:
        return ROUTES[key]
    except KeyError:
        print(f"unknown route: {key}\nknown routes: {', '.join(ORDER)}", file=sys.stderr)
        raise SystemExit(2) from None


def requirements(route: Route) -> tuple[str, ...]:
    """route が必要とする前提。run.py がローカルで充足を判定する。"""
    required = []
    if route.python:
        required.append("python")
    if route.rust:
        required.append("cargo")
    if route.node:
        required.append("node")
    if route.wasm_bindgen:
        required.append("wasm-bindgen")
    if route.java:
        required.append("java")
    if route.gradle:
        required.append("gradle")
    return tuple(required)


def runner_os(route: Route) -> str:
    return RUNNERS[route.os]


def local_os() -> str:
    return "windows" if os.name == "nt" else "linux" if sys.platform.startswith("linux") else "macos"
