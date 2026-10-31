"""path routingと各jobの結果から必須CIの成否を判定する。"""

import json
import os
import sys

from scripts.ci.paths import JOB_IDS


def errors(selected, needs):
    problems = []
    if needs.get("path_filter", {}).get("result") != "success":
        problems.append("path_filter did not succeed")
    if not isinstance(selected, list) or any(route not in JOB_IDS for route in selected):
        return problems + ["invalid path routing output"]
    for route, job in JOB_IDS.items():
        result = needs.get(job, {}).get("result")
        expected = "success" if route in selected else "skipped"
        if result != expected:
            problems.append(f"{job}: expected {expected}, got {result}")
    return problems


def main():
    try:
        selected = json.loads(os.environ["SELECTED_JOBS"])
        needs = json.loads(os.environ["NEEDS_JSON"])
        problems = errors(selected, needs)
    except (KeyError, ValueError, TypeError) as error:
        problems = [f"invalid CI inputs: {error}"]
    for problem in problems:
        print(problem, file=sys.stderr)
    return bool(problems)


if __name__ == "__main__":
    sys.exit(main())
