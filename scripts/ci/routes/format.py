"""Format route: 整形チェックを 1 箇所に集約する。

`cargo fmt --all` は OS 非依存なので Linux で 1 回だけ走らせる。以前は
rust_core / host / desktop で同じ check を 3 回実行していた。
"""

from scripts.ci.model import Route, Step

ROUTE = Route(
    key="format",
    name="Format",
    runs_on=("linux",),
    timeout_minutes=10,
    rust=True,
    rust_components="rustfmt",
    steps=(
        Step("Check workspace formatting", ["cargo", "fmt", "--all", "--", "--check"]),
        Step(
            "Check dev-session formatting",
            ["cargo", "fmt", "--manifest-path", "scripts/dev-session/Cargo.toml", "--", "--check"],
        ),
        Step(
            "Check terminal component formatting",
            ["cargo", "fmt", "--manifest-path", "extensions/terminal/component/Cargo.toml", "--", "--check"],
        ),
    ),
)
