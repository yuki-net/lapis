"""Dependency Policy route: workspace のクレート間依存ホワイトリスト検査。

path routing に関係なく plan が常に選ぶ。許可リストに無い依存の追加だけを
失敗にする（依存の削除や workspace 外への依存は対象外）。
"""

import sys

from scripts.ci.model import PYTHON_VERSION, Route, Step


def py(*args):
    return [sys.executable, *args]


ROUTE = Route(
    key="dependency_policy",
    name="Dependency Policy",
    runs_on=("linux",),
    timeout_minutes=10,
    python=PYTHON_VERSION,
    # cargo metadata --locked --no-deps を使うため。
    rust=True,
    steps=(
        Step("Check dependency allowlist", py("scripts/check_dependencies.py")),
    ),
)
