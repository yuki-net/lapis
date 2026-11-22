"""変更 path から実行すべき route を決め、GitHub の matrix 形式に変換する。

routing の判定は `scripts/ci/paths.py`、route の検証内容は
`scripts/ci/routes/` が正本で、この script は両者を繋いで matrix を作る。
matrix は route × `runs_on` の OS ごとに 1 エントリを出す。
"""

import argparse
import json
import sys

from scripts.ci.console import configure
from scripts.ci.model import RUNNERS, RUST_TOOLCHAIN, local_os, requirements
from scripts.ci.paths import ALL_JOBS, changed_paths, jobs_for_paths, worktree_paths
from scripts.ci.routes import ORDER, ROUTES, job_name, route_for

DEFAULT_BASE = "origin/develop"


def route_keys(paths: list[str] | None) -> list[str]:
    """変更 path から実行すべき route key を ORDER 順で返す。

    Rust のコードが変わるなら、対象 route に加えて `format` と `lint` を
    足す（整形・静的検査は全体に掛けるものなので、path 規則に書かない）。

    paths が None（base が見つからない）場合は全 route を返す。
    """
    selected = ALL_JOBS if paths is None else jobs_for_paths(paths)
    if any(ROUTES[key].rust for key in selected):
        selected = selected | {"format", "lint"}
    return [key for key in ORDER if key in selected]


def entry_for(route, os_name: str) -> dict:
    """1 job 分の matrix エントリ。CI とローカルで同じ形を使う。"""
    return {
        "job": route.key,
        "name": job_name(route, os_name),
        "os": os_name,
        # matrix の値は runner 名。Route.runs_on（OS の集合）とは別物。
        "runner": RUNNERS[os_name],
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


def matrix(keys: list[str], only_os: str | None = None, prefer_os: str | None = None) -> list[dict]:
    """GitHub Actions の matrix に渡す配列。1 エントリ = route × OS。

    - `only_os`: その OS で走るエントリだけに絞る
    - `prefer_os`: route が対応しているならその OS だけにする（無ければ全 OS）。
      develop を Linux に寄せるときに使う。Linux 実装が無い route は
      Windows のまま残るので、Windows 専用の検証が消えない。
    """
    entries = []
    for key in keys:
        route = route_for(key)
        if prefer_os is not None and prefer_os in route.runs_on and not route.always_all_os:
            os_names = (prefer_os,)
        else:
            os_names = tuple(
                os_name
                for os_name in route.runs_on
                if only_os is None or os_name == only_os
            )
        for os_name in os_names:
            entries.append(entry_for(route, os_name))
    return entries


def local_entries(keys: list[str]) -> list[dict]:
    """手元の OS で走らせる job の一覧。

    CI の matrix は route が走る OS ごとに job を作るが、手元は 1 台しか
    ない。他 OS のふりはせず、この機械の OS の step を実行して、その OS 名
    で出す（`runs_on` に無くても実行し、run.py が警告する）。
    """
    os_name = local_os()
    return [entry_for(route_for(key), os_name) for key in keys]


def select_paths(args) -> tuple[str, list[str] | None]:
    """どの範囲を見るかを決める。

    - `--full`: 全部（paths は None）
    - `--worktree`: 未コミットの変更だけ
    - 既定: PR の差分（three-dot）と未コミットの変更の和
    """
    if args.full:
        return "full", None
    if args.worktree:
        return "worktree", worktree_paths()
    return f"{args.base}...HEAD + worktree", changed_paths(args.base)


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
    parser.add_argument(
        "--full",
        "--no-diff",
        dest="full",
        action="store_true",
        help="差分を取らずに全 route を対象にする",
    )
    parser.add_argument(
        "--worktree",
        action="store_true",
        help="コミット済みの差分を無視し、未コミットの変更だけを対象にする",
    )
    parser.add_argument(
        "--only-os",
        choices=("linux", "windows"),
        help="指定した OS で走る matrix エントリだけを出す（runner の切り替え用）",
    )
    parser.add_argument(
        "--prefer-os",
        choices=("linux", "windows"),
        help="route が対応しているならその OS だけにする。Linux 実装が無い route は元の OS のまま",
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

    scope, paths = select_paths(args)
    keys = route_keys(paths)
    entries = matrix(keys, args.only_os, args.prefer_os)
    if args.reason:
        if paths is None:
            detail = "no diff (full run)"
        else:
            detail = f"{len(paths)} changed path(s)"
        suffix = f"; {len(entries)} job(s) on {args.only_os}" if args.only_os else ""
        print(f"{scope}: {detail} select {len(keys)} route(s){suffix}", file=sys.stderr)

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
