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
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch

from scripts.ci import report, run
from scripts.ci.jobs import Route, Step


def route_with(*steps, **kwargs) -> Route:
    return Route(key="test", name="Test", os=kwargs.pop("os", "linux"), timeout_minutes=5, steps=steps, **kwargs)


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
        with patch.object(run, "route_for", return_value=route), \
             patch.object(run, "missing_prerequisites", return_value=["wasm-bindgen"]):
            with redirect_stdout(io.StringIO()):
                self.assertEqual(invoke(["test"]), 0)
        with patch.object(run, "route_for", return_value=route), \
             patch.object(run, "missing_prerequisites", return_value=["wasm-bindgen"]):
            with redirect_stdout(io.StringIO()):
                self.assertEqual(invoke(["test", "--strict"]), 1)

    def test_other_ci_os_still_runs_with_local_toolchain(self):
        """CI の runner OS と違っても、前提が揃えば同じ検証を回す。"""
        route = route_for_os("windows" if os.name != "nt" else "linux")
        with patch.object(run, "route_for", return_value=route), \
             patch.object(run, "missing_prerequisites", return_value=[]), \
             patch.object(run, "execute", return_value={"route": "test", "ok": True, "steps": []}):
            with redirect_stdout(io.StringIO()):
                self.assertEqual(invoke(["test"]), 0)

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
        with redirect_stdout(io.StringIO()) as buffer:
            run.execute(route, None, quiet=True)
        self.assertIn("| step | result | time |", buffer.getvalue())
        self.assertFalse(report.on_github())


def route_for_os(name: str, **kwargs) -> Route:
    return Route(key="test", name="Test", os=name, timeout_minutes=5,
                 steps=(Step("ok", [sys.executable, "-c", "pass"]),), **kwargs)


if __name__ == "__main__":
    unittest.main()
