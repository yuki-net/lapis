"""保護パス判定（scripts/ci/policy.py）のテスト。"""

import io
import os
import sys
import unittest
from contextlib import redirect_stderr, redirect_stdout
from unittest.mock import patch

from scripts.ci import policy


def invoke(argv):
    with patch.object(sys, "argv", ["policy", *argv]):
        return policy.main()


class ProtectedPathTests(unittest.TestCase):
    def test_nested_paths_are_covered(self):
        hits = policy.protected_hits([
            "lints/catalog/file-length.md",
            "lints/fixtures/file-length/fail/too_long.rs",
            ".github/workflows/ci.yml",
            "scripts/ci/routes/host.py",
            "scripts/check_architecture.py",
        ])
        self.assertEqual(len(hits), 5)

    def test_agent_and_policy_files_are_protected(self):
        for path in (
            "AGENTS.md",
            "scripts/architecture-exceptions.json",
            "clippy.toml",
            "rust-toolchain.toml",
            "rustfmt.toml",
        ):
            self.assertIsNotNone(policy.pattern_for(path), path)

    def test_automation_and_reviewer_are_protected(self):
        for path in (
            "automation/policy.toml",
            "automation/roles.toml",
            "reviewer/cli.py",
            "docs/specs/automation.md",
            ".opencode/skills/build-issue/SKILL.md",
            ".opencode/skills/review-pr/SKILL.md",
        ):
            self.assertIsNotNone(policy.pattern_for(path), path)

    def test_product_code_is_not_protected(self):
        for path in (
            "apps/desktop/app/src/main.rs",
            "workbench/state/src/lib.rs",
            "docs/ci.md",
            "host/app/Cargo.toml",
            "Cargo.lock",
        ):
            self.assertIsNone(policy.pattern_for(path), path)

    def test_hits_keep_the_matching_pattern(self):
        hits = policy.protected_hits(["lints/README.md", "scripts/ci/policy.py"])
        self.assertEqual(hits, [
            ("lints/README.md", "lints/*"),
            ("scripts/ci/policy.py", "scripts/ci/*"),
        ])


class BaseResolutionTests(unittest.TestCase):
    def test_argument_wins(self):
        with patch.dict(os.environ, {"LAPIS_POLICY_BASE": "env", "GITHUB_BASE_REF": "main"}):
            self.assertEqual(policy.resolve_base("argument"), "argument")

    def test_environment_then_pull_request_base_then_develop(self):
        with patch.dict(os.environ, {"LAPIS_POLICY_BASE": "env", "GITHUB_BASE_REF": "main"}):
            self.assertEqual(policy.resolve_base(None), "env")
        with patch.dict(os.environ, {"GITHUB_BASE_REF": "main"}, clear=True):
            self.assertEqual(policy.resolve_base(None), "origin/main")
        with patch.dict(os.environ, {}, clear=True):
            self.assertEqual(policy.resolve_base(None), "origin/develop")


class MainTests(unittest.TestCase):
    def test_protected_change_is_noticed_without_failing(self):
        with patch.object(policy, "pr_paths", return_value=["lints/README.md", "apps/web/src/x.ts"]):
            with patch.object(policy.report, "on_github", return_value=True), \
                 patch.object(policy.report, "write_summary") as summary, \
                 redirect_stdout(io.StringIO()) as output, \
                 redirect_stderr(io.StringIO()) as errors:
                self.assertEqual(invoke([]), 0)
        self.assertIn("::notice file=lints/README.md", output.getvalue())
        self.assertNotIn("apps/web", output.getvalue())
        self.assertEqual(errors.getvalue(), "")
        summary.assert_called_once()

    def test_unprotected_change_passes(self):
        with patch.object(policy, "pr_paths", return_value=["apps/web/src/x.ts"]):
            with redirect_stdout(io.StringIO()) as output:
                self.assertEqual(invoke(["--base", "origin/develop"]), 0)
        self.assertIn("変更なし", output.getvalue())

    def test_unresolvable_base_is_an_error(self):
        with patch.object(policy, "pr_paths", return_value=None):
            with redirect_stderr(io.StringIO()) as errors:
                self.assertEqual(invoke(["--base", "missing"]), 2)
        self.assertIn("取得できない", errors.getvalue())


if __name__ == "__main__":
    unittest.main()
