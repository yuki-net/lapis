"""変更 path から実行すべき route を決め、GitHub の matrix 形式に変換する。

routing の判定は既存の scripts/ci/paths.py をそのまま使う。job の定義は
scripts/ci/jobs.py が唯一の正本で、この script は両者を繋ぐだけ。
"""

import argparse
import json
import sys
from pathlib import Path

from scripts.ci.console import configure
from scripts.ci.jobs import ORDER, RUST_TOOLCHAIN, ROUTES, requirements, runner_os, route_for
from scripts.ci.paths import ALL_JOBS, changed_paths, jobs_for_paths

DEFAULT_BASE = "origin/develop"


def route_keys(paths: list[str] | None) -> list[str]:
    """変更 path から実行すべき route key を ORDER 順で返す。

    paths が None（base が見つからない）場合は全 route を返す。
    """
    if paths is None:
        selected = ALL_JOBS | {"repository"}
    else:
        selected = jobs_for_paths(paths)
    return [key for key in ORDER if key in selected]


def matrix(keys: list[str]) -> list[dict]:
    """GitHub Actions の matrix に渡す 1 エントリ/route の配列。"""
    entries = []
    for key in keys:
        route = route_for(key)
        entries.append(
            {
                "job": route.key,
                "name": route.name,
                "runs_on": runner_os(route),
                "timeout": route.timeout_minutes,
                # 以降は ci.yml のセットアップ step の条件に使う。
                "checkout_depth": route.checkout_depth,
                "toolchain": RUST_TOOLCHAIN,
                "python": route.python,
                "node": route.node,
                "node_cache": route.node_cache,
                "node_cache_path": route.node_cache_path,
                "rust": route.rust,
                "rust_components": route.rust_components,
                "rust_targets": route.rust_targets,
                "wasm_bindgen": route.wasm_bindgen,
                "java": route.java,
                "gradle": route.gradle,
                "cache": route.cache,
            }
        )
    return entries


def main() -> int:
    configure()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", default=DEFAULT_BASE, help="diff の基準 ref")
    parser.add_argument("--github-matrix", action="store_true", help="matrix の JSON を stdout に出す")
    parser.add_argument("--selected", action="store_true", help="実行すべき route key の JSON を stdout に出す")
    parser.add_argument("--count", action="store_true", help="実行すべき route 数を stdout に出す")
    parser.add_argument(
        "--github-outputs",
        action="store_true",
        help="GITHUB_OUTPUT へ書く selected/matrix/count を 1 つの JSON で出す",
    )
    parser.add_argument("--reason", action="store_true", help="判定理由を stderr に出す")
    parser.add_argument("--list", action="store_true", help="全 route と前提を表示する")
    parser.add_argument("--no-diff", action="store_true", help="diff を取らずに全 route を対象にする")
    args = parser.parse_args()

    if args.list:
        for key in ORDER:
            route = route_for(key)
            print(f"{key:12} {route.name:24} os={route.os:8} requires={','.join(requirements(route)) or '-'}")
        return 0

    paths = None if args.no_diff else changed_paths(args.base)
    keys = route_keys(paths)
    if args.reason:
        if paths is None:
            print(f"base {args.base} not found; selecting every route", file=sys.stderr)
        else:
            print(f"{len(paths)} changed path(s) select {len(keys)} route(s)", file=sys.stderr)

    if args.github_outputs:
        # git diff を 1 回で済ませ、GITHUB_OUTPUT 用の key=value を出す。
        print(f"selected={json.dumps(keys)}")
        print(f"matrix={json.dumps(matrix(keys))}")
        print(f"count={len(keys)}")
    elif args.github_matrix:
        print(json.dumps(matrix(keys)))
    elif args.selected:
        print(json.dumps(keys))
    elif args.count:
        print(len(keys))
    else:
        print("\n".join(keys))
    return 0


if __name__ == "__main__":
    sys.exit(main())
