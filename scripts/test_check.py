"""共通検証入口の失敗伝播と検査範囲を確認する。"""

import subprocess
import unittest
from unittest.mock import patch

import check


class CheckTests(unittest.TestCase):
    def test_desktop_architecture_runs_boundary_check(self):
        commands = check.commands("desktop", "architecture")
        self.assertTrue(any(command[1:] == ["scripts/check_architecture.py"] for command in commands))

    def test_format_runs_for_both_surfaces_and_propagates_failure(self):
        for surface in ("backend", "desktop"):
            with self.subTest(surface=surface):
                with patch.object(check.subprocess, "run") as execute:
                    execute.side_effect = subprocess.CalledProcessError(1, ["cargo", "fmt"])
                    with self.assertRaises(subprocess.CalledProcessError):
                        check.run(surface, "format")
                    execute.assert_called_once_with(
                        ["cargo", "fmt", "--all", "--", "--check"], cwd=check.ROOT, check=True
                    )

    def test_failure_stops_before_later_checks(self):
        with patch.object(check.subprocess, "run") as execute:
            execute.side_effect = subprocess.CalledProcessError(7, ["cargo", "check"])
            with self.assertRaises(subprocess.CalledProcessError) as error:
                check.run("desktop")
            self.assertEqual(error.exception.returncode, 7)
            self.assertEqual(execute.call_count, 1)
            self.assertEqual(execute.call_args.kwargs, {"cwd": check.ROOT, "check": True})

    def test_backend_and_desktop_keep_ci_scopes_and_lint_errors(self):
        for surface in ("backend", "desktop"):
            for stage in ("check", "test", "lint", "build"):
                with self.subTest(surface=surface, stage=stage):
                    command, = check.commands(surface, stage)
                    self.assertIn("--all-targets", command)
                    self.assertIn("--locked", command)
                    if surface == "backend":
                        self.assertIn("--workspace", command)
                        self.assertEqual(command.count("--exclude"), 2)
                        self.assertIn("lapis", command)
                        self.assertIn("lapis-desktop-ui", command)
                    else:
                        self.assertNotIn("--exclude", command)
                        self.assertIn("lapis", command)
                        if stage != "build":
                            self.assertIn("lapis-desktop-ui", command)
                    if stage == "lint":
                        self.assertEqual(command[-3:], ["--", "-D", "warnings"])

    def test_each_stage_runs_in_order(self):
        with patch.object(check.subprocess, "run") as execute:
            check.run("backend")
        commands = [call.args[0] for call in execute.call_args_list]
        self.assertEqual(commands[0][1:4], ["-m", "unittest", "discover"])
        self.assertEqual(commands[1][1], "scripts/check_architecture.py")
        self.assertEqual([command[1] for command in commands[2:]], ["fmt", "check", "test", "clippy", "build"])


if __name__ == "__main__":
    unittest.main()
