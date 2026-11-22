"""Repository route: Python の静的検査とアーキテクチャ検査。"""

import sys

from scripts.ci.model import PYTHON_VERSION, Route, Step


def py(*args):
    return [sys.executable, *args]


ROUTE = Route(
    key="repository",
    name="Repository",
    runs_on=("linux", "windows"),
    timeout_minutes=20,
    checkout_depth=0,
    python=PYTHON_VERSION,
    steps=(
        Step("Run repository checks", py("scripts/check.py", "repository", "architecture")),
    ),
    # 補助スクリプト・文書・CI 定義。他の route が拾わない path なので、
    # この route が「先」に評価されても問題ない。
)
