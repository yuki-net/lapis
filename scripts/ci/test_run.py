"""scripts/ci/run.py の実行系を、実際のツールチェーンに依存せず検査する。

step の失敗・再試行・前提不足・OS 非対応の扱いを、小さな route を
組み立てて確認する。
"""

import io
import os
import sys
import tempfile
import time
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest.mock import patch

from scripts.ci import model, report, run
from scripts.ci.jobs import Route, Step


def route_with(*steps, **kwargs) -> Route:
    runs_on = kwargs.pop("runs_on", ("linux",))
    return Route(key="test", name="Test", runs_on=runs_on, timeout_minutes=5, steps=steps, **kwargs)


def invoke(argv) -> int:
    with patch.object(sys, "argv", ["run", *argv]):
        return run.main()


class RunTests(unittest.TestCase):
    def test_substitute_expands_runner_temp_and_exe(self):
        temp = tempfile.gettempdir()
        with patch.dict(os.environ, {"RUNNER_TEMP": temp}):
            self.assertEqual(Path(run.substitute("{runner_temp}/a")), Path(temp) / "a")
            self.assertEqual(run.substitute("target{x}.{exe}"), f"target{{x}}.{'.exe' if os.name == 'nt' else ''}")

    def test_timeout_terminates_the_step(self):
        # timeout_minutes は秒換算して使うため、テストでは極小値を渡す。
        step = Step("slow", [sys.executable, "-c", "import time; time.sleep(30)"], timeout_minutes=0.02)
        started = time.perf_counter()
        payload = run.execute(route_with(step), None, quiet=True)
        self.assertFalse(payload["ok"])
        self.assertLess(time.perf_counter() - started, 20)

    def test_grandchild_holding_output_does_not_hang_the_step(self):
        """孫プロセスが出力を握ったままでも step が待ち続けないこと。

        パイプを渡すと EOF 待ちで 18 分止まった（CI の desktop terminal pty）。
        出力は一時ファイルへ書かせる。
        """
        script = (
            "import subprocess, sys\n"
            "subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)'])\n"
            "print('parent done')\n"
        )
        step = Step("spawns-grandchild", [sys.executable, "-c", script], timeout_minutes=0.2)
        started = time.perf_counter()
        payload = run.execute(route_with(step), None, quiet=True)
        elapsed = time.perf_counter() - started
        self.assertTrue(payload["ok"], payload)
        self.assertLess(elapsed, 15)

    def test_step_failure_stops_the_route(self):
        route = route_with(
            Step("fail", [sys.executable, "-c", "import sys; print('boom'); sys.exit(3)"]),
            Step("never", [sys.executable, "-c", "print('unreachable')"]),
        )
        payload = run.execute(route, None, quiet=True)
        self.assertFalse(payload["ok"])
        self.assertEqual([step["name"] for step in payload["steps"]], ["fail"])
        self.assertFalse(route.steps[1].name in [step["name"] for step in payload["steps"]])

    def test_retries_until_success(self):
        script = "import pathlib,sys; p=pathlib.Path({path!r}); n=p.read_text() if p.exists() else ''; p.write_text(n+'x'); sys.exit(0 if len(n)>=1 else 1)"
        with tempfile.TemporaryDirectory() as directory:
            marker = Path(directory) / "attempts"
            route = route_with(
                Step("flaky", [sys.executable, "-c", script.format(path=str(marker))], retries=2),
            )
            payload = run.execute(route, None, quiet=True)
        self.assertTrue(payload["ok"])

    def test_missing_prerequisite_is_reported_not_run(self):
        route_with_prereq = route_with(Step("needs-gradle", ["gradle", "build"], requires=("gradle",)))
        with patch.object(run.shutil, "which", return_value=None):
            missing = run.missing_prerequisites_for_step(route_with_prereq.steps[0])
        self.assertEqual(missing, ["gradle"])

    def test_missing_prerequisite_skips_without_failing(self):
        route = route_for_os("linux")
        # 前提確認はローカル専用。CI 上でテストするため明示的に切る。
        with patch.object(run.report, "on_github", return_value=False):
            with patch.object(run, "local_os", return_value="linux"), \
                 patch.object(run, "route_for", return_value=route), \
                 patch.object(run, "missing_prerequisites", return_value=["wasm-bindgen"]):
                with redirect_stdout(io.StringIO()):
                    self.assertEqual(invoke(["test"]), 0)
            with patch.object(run, "local_os", return_value="linux"), \
                 patch.object(run, "route_for", return_value=route), \
                 patch.object(run, "missing_prerequisites", return_value=["wasm-bindgen"]):
                with redirect_stdout(io.StringIO()):
                    self.assertEqual(invoke(["test", "--strict"]), 1)

    def test_local_os_outside_runs_on_is_refused(self):
        """CI で走らない OS では、黙って別 OS の検証を回さない。"""
        route = route_for_os("windows")
        with patch.object(run.report, "on_github", return_value=False), \
             patch.object(run, "local_os", return_value="linux"), \
             patch.object(run, "route_for", return_value=route), \
             patch.object(run, "execute") as execute:
            with redirect_stderr(io.StringIO()) as errors:
                self.assertEqual(invoke(["test"]), 1)
        execute.assert_not_called()
        self.assertIn("runs on windows in CI", errors.getvalue())

    def test_os_flag_allows_running_a_ci_os_locally(self):
        """`--os` を明示したときだけ、CI と違う OS で回せる。"""
        route = route_for_os("windows")
        with patch.object(run.report, "on_github", return_value=False), \
             patch.object(run, "local_os", return_value="linux"), \
             patch.object(run, "route_for", return_value=route), \
             patch.object(run, "missing_prerequisites", return_value=[]), \
             patch.object(run, "execute", return_value={"route": "test", "ok": True, "steps": []}):
            with redirect_stdout(io.StringIO()):
                self.assertEqual(invoke(["test", "--os", "windows"]), 0)

    def test_os_flag_rejects_unknown_os(self):
        with redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit):
                invoke(["test", "--os", "solaris"])

    def test_platform_steps_are_loaded_by_file_name(self):
        """`<route>_<os>.py` を置くだけで OS 別 step が載る。"""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "widget_windows.py").write_text(
                "from scripts.ci.model import Step\n"
                "STEPS = (Step('win-only', ['cmd']),)\n",
                encoding="utf-8",
            )
            (root / "widget_linux.py").write_text(
                "from scripts.ci.model import Step\n"
                "STEPS = (Step('nix-only', ['sh']),)\n",
                encoding="utf-8",
            )
            (root / "other_windows.py").write_text("STEPS = ()\n", encoding="utf-8")
            loaded = model.load_platform_steps(root, "widget")
        self.assertEqual(sorted(loaded), ["linux", "windows"])
        self.assertEqual(loaded["windows"][0].name, "win-only")
        self.assertEqual(loaded["linux"][0].name, "nix-only")

    def test_platform_steps_reject_unknown_os_file_name(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "widget_solaris.py").write_text("STEPS = ()\n", encoding="utf-8")
            with self.assertRaises(ValueError) as raised:
                model.load_platform_steps(root, "widget")
        self.assertIn("solaris", str(raised.exception))

    def test_platform_steps_require_steps_definition(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "widget_windows.py").write_text("OTHER = ()\n", encoding="utf-8")
            with self.assertRaises(ValueError) as raised:
                model.load_platform_steps(root, "widget")
        self.assertIn("STEPS", str(raised.exception))

    def test_execute_runs_only_the_selected_os_platform_steps(self):
        """選んだ OS の差分 step だけを、共通 step の後ろに足す。"""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "test_windows.py").write_text(
                "from scripts.ci.model import Step\n"
                "STEPS = (Step('win-only', ['cmd']),)\n",
                encoding="utf-8",
            )
            (root / "test_linux.py").write_text(
                "from scripts.ci.model import Step\n"
                "STEPS = (Step('nix-only', ['sh']),)\n",
                encoding="utf-8",
            )
            route = route_with(Step("common", [sys.executable, "-c", "print('ok')"]))

            def fake_step(_route, step, _log):
                return {"name": step.name, "ok": True, "seconds": 0.0, "output": ""}

            with patch.object(model, "PLATFORM_DIRECTORY", root), \
                 patch.object(report, "on_github", return_value=False), \
                 patch.object(run, "run_step", side_effect=fake_step):
                windows = run.execute(route, None, quiet=True, os_name="windows")
                linux = run.execute(route, None, quiet=True, os_name="linux")
        self.assertEqual([step["name"] for step in windows["steps"]], ["common", "win-only"])
        self.assertEqual([step["name"] for step in linux["steps"]], ["common", "nix-only"])

    def test_step_selection_rejects_unknown_names(self):
        with patch.object(run, "route_for", return_value=route_for_os("linux")):
            with redirect_stdout(io.StringIO()):
                with self.assertRaises(SystemExit):
                    run.execute(route_for_os("linux"), ["nope"], quiet=True)

    def test_json_payload_has_no_step_output(self):
        route = route_with(Step("ok", [sys.executable, "-c", "print('hello')"]))
        payload = run.execute(route, None, quiet=True)
        self.assertTrue(payload["ok"])
        step = payload["steps"][0]
        self.assertNotIn("output", step)
        self.assertEqual(set(step), {"name", "ok", "seconds"})

    def test_local_summary_is_written_to_stdout_only(self):
        route = route_with(Step("ok", [sys.executable, "-c", "pass"]))
        # CI 上では GitHub 向けの出力になるため、ローカル表示を明示する。
        with patch.object(report, "on_github", return_value=False):
            with redirect_stdout(io.StringIO()) as buffer:
                run.execute(route, None, quiet=True)
        self.assertIn("| step | result | time |", buffer.getvalue())


def route_for_os(name: str, **kwargs) -> Route:
    return Route(key="test", name="Test", runs_on=(name,), timeout_minutes=5,
                 steps=(Step("ok", [sys.executable, "-c", "pass"]),), **kwargs)


if __name__ == "__main__":
    unittest.main()
