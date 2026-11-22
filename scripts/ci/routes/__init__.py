"""route の集約。ここは「どの route がどの順で走るか」だけを持つ。

各 route の検証内容は `scripts/ci/routes/<key>.py` が 1 route 1 ファイルで
持ち、どの変更 path でどの route を走らせるかは route 同士の関係なので
`scripts/ci/paths.py` が 1 箇所で持つ。
"""

import sys

from scripts.ci.model import OS_LABELS, RUNNERS, Route
from scripts.ci.routes import (
    desktop,
    dev_commands,
    format as format_route,
    host,
    lint,
    mobile,
    repository,
    rust_core,
    sites_docs,
    web,
)

# 実行順。matrix の並び順もここに従う。
ORDER = (
    "repository",
    "format",
    "lint",
    "dev_commands",
    "rust_core",
    "host",
    "desktop",
    "mobile",
    "web",
    "sites_docs",
)

ROUTES: dict[str, Route] = {
    module.ROUTE.key: module.ROUTE
    for module in (
        repository,
        format_route,
        lint,
        dev_commands,
        rust_core,
        host,
        desktop,
        mobile,
        web,
        sites_docs,
    )
}


def validate(routes: dict[str, Route]) -> None:
    """`runs_on` が実際に runner を持つ OS だけを指しているか検査する。

    ここを素通りさせると、matrix を作るときに `RUNNERS[os_name]` で
    KeyError になり、原因が分からないまま plan が落ちる。
    """
    for route in routes.values():
        if not route.runs_on:
            raise ValueError(f"{route.key}: runs_on を空にしない")
        unknown = [os_name for os_name in route.runs_on if os_name not in RUNNERS]
        if unknown:
            raise ValueError(
                f"{route.key}: unknown OS {', '.join(unknown)} "
                f"(expected {', '.join(RUNNERS)})"
            )


validate(ROUTES)


def route_for(key: str) -> Route:
    try:
        return ROUTES[key]
    except KeyError:
        print(f"unknown route: {key}\nknown routes: {', '.join(ORDER)}", file=sys.stderr)
        raise SystemExit(2) from None


def job_name(route: Route, os_name: str) -> str:
    """GitHub の job 名。`name` に OS を混ぜず、ここで組み立てる。

    検証対象が runner の OS と違う場合（Mobile / Android）は `target` を
    挟む。複数 OS で走る route は OS を後ろに足して区別する
    （`Mobile / Android / Windows`）。
    """
    parts = [route.name]
    if route.target:
        parts.append(route.target)
        if len(route.runs_on) > 1:
            parts.append(OS_LABELS[os_name])
    else:
        parts.append(OS_LABELS[os_name])
    return " / ".join(parts)
