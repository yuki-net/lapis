"""scripts/ci/plan.py と jobs.py の整合を検査する。

job 定義を jobs.py に集約したので、workflow 側が matrix を展開するだけで
済むことを保証するテストを置く。YAML に job を書き足すドリフトを防ぐ。
"""

import json
import re
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.ci import plan
from scripts.ci.jobs import ORDER, ROOT, ROUTES, RUST_TOOLCHAIN, route_for
from scripts.ci.paths import PATH_RULES

WORKFLOW = ROOT / ".github/workflows/ci.yml"


class PlanTests(unittest.TestCase):
    def test_route_keys_follow_order(self):
        self.assertEqual(plan.route_keys(None), list(ORDER))
        keys = plan.route_keys(["clients/desktop/app/src/main.rs"])
        self.assertEqual(keys, [key for key in ORDER if key in {"desktop"}])
        self.assertEqual(keys, sorted(keys, key=ORDER.index))

    def test_unknown_path_selects_every_route(self):
        self.assertEqual(plan.route_keys(["unknown-file.txt"]), list(ORDER))

    def test_matrix_entries_only_reference_known_routes(self):
        entries = plan.matrix(list(ORDER))
        self.assertEqual([entry["job"] for entry in entries], list(ORDER))
        for entry in entries:
            route = route_for(entry["job"])
            self.assertEqual(entry["name"], route.name)
            self.assertEqual(entry["timeout"], route.timeout_minutes)
            self.assertEqual(entry["runs_on"], "windows-2022" if route.os == "windows" else "ubuntu-latest")
            self.assertEqual(entry["workdir"], route.working_directory)
            json.dumps(entry)  # matrix は JSON へ直列化できること

    def test_matrix_entries_avoid_shell_injection(self):
        """step は argv で持つので、matrix に shell 文字列を渡さない。"""
        for entry in plan.matrix(list(ORDER)):
            self.assertNotIn("shell", entry)

    def test_matrix_carries_the_pinned_toolchain(self):
        """toolchain は jobs.py の定数を matrix で運び、YAML に直書きしない。"""
        workflow = WORKFLOW.read_text(encoding="utf-8")
        self.assertIn("toolchain: ${{ matrix.route.toolchain }}", workflow)
        for entry in plan.matrix(list(ORDER)):
            self.assertEqual(entry["toolchain"], RUST_TOOLCHAIN)

    def test_matrix_is_json_serializable_with_empty_selection(self):
        self.assertEqual(plan.matrix([]), [])

    def test_workflow_defines_only_plan_ci_gate(self):
        workflow = WORKFLOW.read_text(encoding="utf-8")
        jobs = re.findall(r"(?m)^  ([a-z_]+):\s*$", workflow.split("\njobs:\n", 1)[1])
        self.assertEqual(jobs, ["plan", "ci", "gate"])

    def test_workflow_delegates_every_route_to_run_py(self):
        """step 列を YAML に書かず、run.py に委譲していること。"""
        workflow = WORKFLOW.read_text(encoding="utf-8")
        self.assertIn("python -m scripts.ci.run ${{ matrix.route.job }}", workflow)
        for step in ("python scripts/check.py", "cargo test", "npm run build", "./gradlew"):
            self.assertNotIn(step, workflow)

    def test_workflow_matrix_comes_from_plan(self):
        workflow = WORKFLOW.read_text(encoding="utf-8")
        self.assertIn("matrix:\n        route: ${{ fromJSON(needs.plan.outputs.matrix) }}", workflow)

    def test_every_route_key_is_reachable_from_paths(self):
        """全 route が path routing から選ばれ得る（選ばれない route は定義ミス）。"""
        reachable = set()
        for _, jobs in PATH_RULES:
            reachable.update(jobs)
        self.assertEqual(reachable, set(ROUTES))

    def test_cli_outputs_match_the_library(self):
        import io
        from contextlib import redirect_stdout

        buffer = io.StringIO()
        with patch("sys.argv", ["plan", "--selected", "--no-diff"]), redirect_stdout(buffer):
            self.assertEqual(plan.main(), 0)
        self.assertEqual(json.loads(buffer.getvalue()), list(ORDER))
        self.assertEqual(plan.matrix(["desktop"])[0]["job"], "desktop")


if __name__ == "__main__":
    unittest.main()
