"""plan と matrix の結果から CI の成否を判定する。

matrix 化により個々の route の結果は gate から見えないため、判定材料は
「plan が選んだ route」と「matrix 全体の結果」になる。route ごとの
期待値（実行 or 未実行）は plan.py のユニットテストで担保する。
"""

import json
import os
import sys

from scripts.ci.jobs import ORDER


def errors(selected, needs, count):
    problems = []
    if needs.get("plan", {}).get("result") != "success":
        problems.append("plan did not succeed")
    if not isinstance(selected, list) or any(route not in ORDER for route in selected):
        return problems + ["invalid path routing output"]
    if selected and count != len(selected):
        problems.append(f"plan selected {len(selected)} route(s) but reported count={count}")
    result = needs.get("ci", {}).get("result")
    expected = "success" if selected else "skipped"
    if result != expected:
        problems.append(f"ci: expected {expected}, got {result}")
    return problems


def main() -> int:
    try:
        selected = json.loads(os.environ["SELECTED_ROUTES"])
        count = int(os.environ["SELECTED_COUNT"])
        needs = json.loads(os.environ["NEEDS_JSON"])
        problems = errors(selected, needs, count)
    except (KeyError, ValueError, TypeError) as error:
        problems = [f"invalid CI inputs: {error}"]
    for problem in problems:
        print(problem, file=sys.stderr)
    return bool(problems)


if __name__ == "__main__":
    sys.exit(main())
