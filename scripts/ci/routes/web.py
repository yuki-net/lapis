"""Web route: Web クライアントのビルドと接続処理テスト。

npm は OS で実行ファイル名が違う（`npm` / `npm.cmd`）ため `{npm}` を使う。
"""

from scripts.ci.model import NODE_VERSION, WASM_BINDGEN_VERSION, Route, Step

ROUTE = Route(
    key="web",
    name="Web",
    runs_on=("linux", "windows"),
    timeout_minutes=20,
    node=NODE_VERSION,
    node_cache="npm",
    node_cache_path="clients/web/package-lock.json",
    rust=True,
    rust_targets="wasm32-unknown-unknown",
    wasm_bindgen=WASM_BINDGEN_VERSION,
    working_directory="clients/web",
    steps=(
        Step("Install dependencies", ["{npm}", "ci"]),
        Step("Build Web", ["{npm}", "run", "build"]),
        Step("Test Web connection handling", ["{npm}", "test"]),
    ),
)
