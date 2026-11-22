"""Lint route: clippy を OS ごとに 1 回だけ走らせる。

clippy は `RUSTC_WRAPPER` が変わるためほぼフル再コンパイルになる。領域ごとに
分けると同じ依存を何度もコンパイルするので、OS 単位でまとめる。

- Linux: desktop（GPUI）はシステムライブラリが要るため backend に絞る
- Windows: workspace 全体（host / desktop / launcher を含む）
"""

from scripts.ci.model import Route, Step

BACKEND_SCOPE = [
    "--workspace",
    "--all-targets",
    "--exclude",
    "lapis",
    "--exclude",
    "desktop-ui",
    "--exclude",
    "lapis-launcher",
    "--exclude",
    "host",
]

ROUTE = Route(
    key="lint",
    name="Lint",
    # develop でも全 OS で見る（`cfg(windows)` のコードは Windows でしか検査できない）。
    runs_on=("linux", "windows"),
    always_all_os=True,
    timeout_minutes=45,
    rust=True,
    rust_components="clippy",
    cache=True,
    steps=(
        Step(
            "Lint backend",
            ["cargo", "clippy", "--locked", *BACKEND_SCOPE, "--", "-D", "warnings"],
            requires=("posix",),
        ),
        Step(
            "Lint all crates",
            ["cargo", "clippy", "--locked", "--workspace", "--all-targets", "--", "-D", "warnings"],
            requires=("windows",),
        ),
    ),
)
