"""Host route: Windows の Host 検証（test / build と Web 公開の検証）。"""

import sys

from scripts.ci.model import NODE_VERSION, PYTHON_VERSION, WASM_BINDGEN_VERSION, Route, Step


def py(*args):
    return [sys.executable, *args]


ROUTE = Route(
    key="host",
    name="Host",
    runs_on=("windows",),
    # Host の test / build と Web 公開の検証で約 30 分。承認済み拡張の
    # native / PTY 検証は extensions route が持つため、ここには含めない。
    # job の後処理で target/ のキャッシュ保存に数分かかるので、上限は
    # 保存分を含めて決める。
    timeout_minutes=45,
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
