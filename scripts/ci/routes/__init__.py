"""route の集約。ここは「どの route がどの順で走るか」だけを持つ。

各 route の検証内容は `scripts/ci/routes/<key>.py` が 1 route 1 ファイルで
持ち、どの変更 path でどの route を走らせるかは route 同士の関係なので
`scripts/ci/paths.py` が 1 箇所で持つ。
"""

import sys

from scripts.ci.model import OS_LABELS, Route
from scripts.ci.routes import desktop, dev_commands, host, mobile, repository, rust_core, web

# 実行順。matrix の並び順もここに従う。
ORDER = ("repository", "dev_commands", "rust_core", "host", "desktop", "mobile", "web")

ROUTES: dict[str, Route] = {
    module.ROUTE.key: module.ROUTE
    for module in (repository, dev_commands, rust_core, host, desktop, mobile, web)
}


def route_for(key: str) -> Route:
    try:
        return ROUTES[key]
    except KeyError:
        print(f"unknown route: {key}\nknown routes: {', '.join(ORDER)}", file=sys.stderr)
        raise SystemExit(2) from None


def job_name(route: Route, os_name: str) -> str:
    """GitHub の job 名。`name` に OS を混ぜず、ここで組み立てる。

    検証対象が runner の OS と違う場合（Mobile / Android）は `target` を
    使う。複数 OS で走る route は job 名の重複を避けるため OS を優先する。
    """
    if route.target and len(route.runs_on) == 1:
        return f"{route.name} / {route.target}"
    return f"{route.name} / {OS_LABELS[os_name]}"
