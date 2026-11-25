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
from scripts.ci.model import ROOT, RUST_TOOLCHAIN
from scripts.ci.routes import ORDER, ROUTES, route_for
from scripts.ci.model import RUNNERS
from scripts.ci.paths import PATH_RULES
from scripts.ci.routes import job_name

WORKFLOW = ROOT / ".github/workflows/ci.yml"


class PlanTests(unittest.TestCase):
    def test_route_keys_follow_order(self):
        self.assertEqual(plan.route_keys(None), list(ORDER))
        keys = plan.route_keys(["apps/desktop/app/src/main.rs"])
        self.assertEqual(keys, [key for key in ORDER if key in {"desktop", "format", "lint"}])
        self.assertEqual(keys, sorted(keys, key=ORDER.index))

    def test_unknown_path_selects_every_route(self):
        self.assertEqual(plan.route_keys(["unknown-file.txt"]), list(ORDER))

    def test_matrix_entries_only_reference_known_routes(self):
        entries = plan.matrix(list(ORDER))
        # 1 route が複数 OS で走る場合は route × OS のエントリになる。
        self.assertEqual(list(dict.fromkeys(entry["job"] for entry in entries)), list(ORDER))
        self.assertEqual(len(entries), sum(len(route_for(key).runs_on) for key in ORDER))
        for entry in entries:
            route = route_for(entry["job"])
            self.assertIn(entry["os"], route.runs_on)
            expected = job_name(route, entry["os"])
            self.assertEqual(entry["name"], expected)
            self.assertEqual(entry["timeout"], route.timeout_minutes)
            self.assertEqual(entry["runner"], RUNNERS[entry["os"]])
            self.assertIs(entry["cache"], route.cache)
            self.assertNotIn("runs_on", entry)
            self.assertNotIn("cache_paths", entry)
            self.assertNotIn("workdir", entry)
            self.assertNotIn("requires", entry)
            json.dumps(entry)  # matrix は JSON へ直列化できること

    def test_matrix_expands_every_os_a_route_runs_on(self):
        """`runs_on` の全 OS が matrix のエントリになる。"""
        for key in ORDER:
            route = route_for(key)
            entries = plan.matrix([key])
            self.assertEqual([entry["os"] for entry in entries], list(route.runs_on), key)

    def test_matrix_can_be_filtered_by_os(self):
        """runner を切り替えるときは `--only-os` で OS を絞る。"""
        entries = plan.matrix(list(ORDER), only_os="linux")
        self.assertTrue(entries)
        self.assertEqual({entry["os"] for entry in entries}, {"linux"})
        self.assertEqual([entry["job"] for entry in entries], [
            key for key in ORDER if "linux" in route_for(key).runs_on
        ])
        windows = [entry["job"] for entry in plan.matrix(list(ORDER), only_os="windows")]
        self.assertEqual(windows, [key for key in ORDER if "windows" in route_for(key).runs_on])
        self.assertEqual(plan.matrix([], only_os="linux"), [])

    def test_prefer_os_keeps_one_job_per_route_except_always_all_os(self):
        """develop は Linux に寄せる。全 OS で見たい route だけは両方残す。"""
        entries = plan.matrix(list(ORDER), prefer_os="linux")
        for route in ROUTES.values():
            got = sorted(entry["os"] for entry in entries if entry["job"] == route.key)
            if route.always_all_os:
                self.assertEqual(got, sorted(route.runs_on), route.key)
            else:
                expected = ["linux"] if "linux" in route.runs_on else [route.runs_on[0]]
                self.assertEqual(got, expected, route.key)

    def test_job_name_uses_target_when_the_runner_os_is_not_the_subject(self):
        """Mobile は Android を検証するので job 名に対象を出す。"""
        self.assertEqual(plan.matrix(["mobile"])[0]["name"], "Mobile / Android / Linux")
        self.assertEqual(plan.matrix(["mobile"])[1]["name"], "Mobile / Android / Windows")
        self.assertEqual(plan.matrix(["host"])[0]["name"], "Host / Windows")
        self.assertEqual(plan.matrix(["repository"])[0]["name"], "Repository / Linux")

    def test_routes_declare_only_known_os(self):
        """`runs_on` の typo は matrix を作る前に落とす。"""
        from dataclasses import replace

        from scripts.ci.routes import validate

        route = route_for("web")
        validate({"ok": route})
        with self.assertRaises(ValueError) as raised:
            validate({"bad": replace(route, runs_on=("solaris",))})
        self.assertIn("solaris", str(raised.exception))
        with self.assertRaises(ValueError):
            validate({"empty": replace(route, runs_on=())})

    def test_matrix_carries_working_directory_indirectly(self):
        """作業ディレクトリは matrix ではなく run.py が route から解決する。"""
        workflow = WORKFLOW.read_text(encoding="utf-8")
        self.assertNotIn("matrix.route.workdir", workflow)
        self.assertNotIn("matrix.route.requires", workflow)

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

    def test_lint_collects_diagnostics_instead_of_failing_early(self):
        """lint は -D warnings で止めず、JSON 診断を集めてから失敗させる。"""
        for step in route_for("lint").steps:
            self.assertTrue(step.json_diagnostics, step.name)
            self.assertIn("--message-format=json", step.command)
            self.assertIn("--keep-going", step.command)
            self.assertNotIn("-D", step.command)

    def test_matrix_is_json_serializable_with_empty_selection(self):
        self.assertEqual(plan.matrix([]), [])

    def test_workflow_defines_only_plan_ci_policy_gate(self):
        workflow = WORKFLOW.read_text(encoding="utf-8")
        jobs = re.findall(r"(?m)^  ([a-z_]+):\s*$", workflow.split("\njobs:\n", 1)[1])
        self.assertEqual(jobs, ["plan", "ci", "policy", "gate"])

    def test_policy_runs_only_on_pull_requests(self):
        """保護パスは PR のときだけ通知する。マージのゲートは CODEOWNERS。"""
        workflow = WORKFLOW.read_text(encoding="utf-8")
        self.assertIn("if: github.event_name == 'pull_request'", workflow)
        self.assertIn("needs: [plan, ci]", workflow)
        self.assertIn("python -m scripts.ci.policy", workflow)

    def test_workflow_delegates_every_route_to_run_py(self):
        """step 列を YAML に書かず、run.py に委譲していること。"""
        workflow = WORKFLOW.read_text(encoding="utf-8")
        self.assertIn("python -m scripts.ci.run ${{ matrix.route.job }}", workflow)
        for step in ("python scripts/check.py", "cargo test", "npm run build", "./gradlew"):
            self.assertNotIn(step, workflow)

    def test_workflow_passes_the_matrix_os_to_run_py(self):
        """CI は matrix の OS を `--os` で run.py に渡す。"""
        workflow = WORKFLOW.read_text(encoding="utf-8")
        self.assertIn("--os ${{ matrix.route.os }}", workflow)

    def test_workflow_uses_the_matrix_runner_and_rust_cache(self):
        """YAML は runner 名を matrix から受け、Rust は rust-cache に任せる。"""
        workflow = WORKFLOW.read_text(encoding="utf-8")
        self.assertIn("runs-on: ${{ matrix.route.runner }}", workflow)
        self.assertIn("Swatinem/rust-cache@v2", workflow)
        self.assertIn("shared-key: rust-${{ matrix.route.os }}", workflow)
        # 計測 step は du / find を使うので、Windows でも bash で動かす。
        self.assertIn("name: Report restored cache", workflow)
        self.assertIn("shell: bash", workflow)
        self.assertNotIn("matrix.route.runs_on", workflow)
        self.assertNotIn("actions/cache@", workflow)

    def test_workflow_has_a_manual_full_run_input(self):
        """手動実行では、差分を見ずに全 route・全 OS で回せる。"""
        workflow = WORKFLOW.read_text(encoding="utf-8")
        self.assertIn("inputs.full", workflow)
        self.assertIn('"${{ inputs.full }}" = "true"', workflow)

    def test_workflow_scopes_main_and_develop_differently(self):
        """main は全 route、develop は Linux 優先。"""
        workflow = WORKFLOW.read_text(encoding="utf-8")
        self.assertIn('if [ "${BASE_REF}" = "main" ]', workflow)
        self.assertIn("scope=(--full)", workflow)
        self.assertIn("scope=(--prefer-os linux)", workflow)

    def test_workflow_matrix_comes_from_plan(self):
        workflow = WORKFLOW.read_text(encoding="utf-8")
        self.assertIn("matrix:\n        route: ${{ fromJSON(needs.plan.outputs.matrix) }}", workflow)

    def test_every_route_key_is_reachable_from_paths(self):
        """全 route が path routing から選ばれ得る（選ばれない route は定義ミス）。"""
        reachable = set()
        for _, jobs in PATH_RULES:
            reachable.update(jobs)
        # format / lint は path 規則ではなく rust route から自動で足す。
        reachable.update({"format", "lint"})
        self.assertEqual(reachable, set(ROUTES))

    def test_cli_outputs_match_the_library(self):
        import io
        from contextlib import redirect_stdout

        buffer = io.StringIO()
        with patch("sys.argv", ["plan", "--selected", "--no-diff"]), redirect_stdout(buffer):
            self.assertEqual(plan.main(), 0)
        self.assertEqual(json.loads(buffer.getvalue()), list(ORDER))
        self.assertEqual(plan.matrix(["desktop"])[0]["job"], "desktop")

    def test_full_mode_selects_every_route_without_touching_git(self):
        import io
        from contextlib import redirect_stdout

        buffer = io.StringIO()
        with patch.object(plan, "changed_paths") as diff, patch.object(plan, "worktree_paths") as worktree, \
             patch("sys.argv", ["plan", "--selected", "--full"]), redirect_stdout(buffer):
            self.assertEqual(plan.main(), 0)
        diff.assert_not_called()
        worktree.assert_not_called()
        self.assertEqual(json.loads(buffer.getvalue()), list(ORDER))

    def test_worktree_mode_ignores_committed_changes(self):
        import io
        from contextlib import redirect_stdout

        buffer = io.StringIO()
        with patch.object(plan, "worktree_paths", return_value=["docs/ci.md"]), \
             patch("sys.argv", ["plan", "--selected", "--worktree"]), redirect_stdout(buffer):
            self.assertEqual(plan.main(), 0)
        self.assertEqual(json.loads(buffer.getvalue()), ["repository"])

    def test_default_mode_combines_pull_request_and_worktree(self):
        with patch.object(plan, "changed_paths", return_value=["apps/web/package.json"]) as changed:
            scope, paths = plan.select_paths(
                type("Args", (), {"full": False, "worktree": False, "base": "origin/develop"})()
            )
        changed.assert_called_once_with("origin/develop")
        self.assertEqual(paths, ["apps/web/package.json"])
        self.assertIn("origin/develop", scope)

    def test_unknown_base_falls_back_to_every_route(self):
        import io
        from contextlib import redirect_stdout

        buffer = io.StringIO()
        with patch.object(plan, "changed_paths", return_value=None), \
             patch("sys.argv", ["plan", "--selected"]), redirect_stdout(buffer):
            self.assertEqual(plan.main(), 0)
        self.assertEqual(json.loads(buffer.getvalue()), list(ORDER))


if __name__ == "__main__":
    unittest.main()
