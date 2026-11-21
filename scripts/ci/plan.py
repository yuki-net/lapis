"""変更 path から実行すべき route を決め、GitHub の matrix 形式に変換する。

routing の判定は `scripts/ci/paths.py`、route の検証内容は
`scripts/ci/routes/` が正本で、この script は両者を繋いで matrix を作る。
matrix は route × `runs_on` の OS ごとに 1 エントリを出す。
"""

import argparse
import json
import sys
from pathlib import Path

from scripts.ci.console import configure
from scripts.ci.model import RUNNERS
from scripts.ci.routes import ORDER, job_name
from scripts.ci.jobs import RUST_TOOLCHAIN, ROUTES, requirements, route_for
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


def matrix(keys: list[str], only_os: str | None = None) -> list[dict]:
    """GitHub Actions の matrix に渡す配列。1 エントリ = route × OS。

    only_os を渡すと、その OS で走るエントリだけに絞る（runner を
    切り替えたいときに使う）。
    """
    entries = []
    for key in keys:
        route = route_for(key)
        for os_name in route.runs_on:
            if only_os is not None and os_name != only_os:
                continue
            entries.append(
                {
                    "job": route.key,
                    "name": job_name(route, os_name),
                    "os": os_name,
                    "runs_on": RUNNERS[os_name],
                    "timeout": route.timeout_minutes,
                    # watch.py が待機中の job の総 step 数を出すのに使う。
                    "steps": len(route.steps) + len(route.platform_steps.get(os_name, ())),
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
    parser.add_argument(
        "--only-os",
        choices=("linux", "windows"),
        help="指定した OS で走る matrix エントリだけを出す（runner の切り替え用）",
    )
    args = parser.parse_args()

    if args.list:
        for key in ORDER:
            route = route_for(key)
            print(
                f"{key:12} {route.name:16} runs_on={','.join(route.runs_on):18} "
                f"requires={','.join(requirements(route)) or '-'}"
            )
        return 0

    paths = None if args.no_diff else changed_paths(args.base)
    keys = route_keys(paths)
    entries = matrix(keys, args.only_os)
    if args.reason:
        if paths is None:
            print(f"base {args.base} not found; selecting every route", file=sys.stderr)
        elif args.only_os:
            print(
                f"{len(paths)} changed path(s) select {len(keys)} route(s); "
                f"{len(entries)} job(s) run on {args.only_os}",
                file=sys.stderr,
            )
        else:
            print(f"{len(paths)} changed path(s) select {len(keys)} route(s)", file=sys.stderr)

    if args.github_outputs:
        # git diff を 1 回で済ませ、GITHUB_OUTPUT 用の key=value を出す。
        print(f"selected={json.dumps(keys)}")
        print(f"matrix={json.dumps(entries)}")
        print(f"count={len(keys)}")
    elif args.github_matrix:
        print(json.dumps(entries))
    elif args.selected:
        print(json.dumps(keys))
    elif args.count:
        print(len(keys))
    else:
        print("\n".join(keys))
    return 0


if __name__ == "__main__":
    sys.exit(main())
