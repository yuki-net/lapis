"""scripts/ci/run.py の実行系を、実際のツールチェーンに依存せず検査する。

step の失敗・再試行・前提不足・OS 非対応の扱いを、小さな route を
組み立てて確認する。
"""

import io
import json
import os
import sys
import tempfile
import time
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest.mock import patch

from scripts.ci import model, report, run, state
from scripts.ci.model import Route, Step


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

    def test_substitute_expands_os_specific_command_names(self):
        """OS で名前が変わるコマンドだけを 1 つの step 列に書ける。"""
        self.assertEqual(run.substitute("{npm} ci", "linux"), "npm ci")
        self.assertEqual(run.substitute("{npm} ci", "windows"), "npm.cmd ci")
        self.assertEqual(run.substitute("{gradlew}", "linux"), "./gradlew")
        self.assertEqual(run.substitute("{gradlew}", "windows"), "gradlew.bat")

    def test_python_steps_get_utf8_io_encoding(self):
        """cp1252 の runner で日本語 print が UnicodeEncodeError にならないようにする。"""
        environment = run.step_environment(Step("x", ["python", "-c", "pass"]), "windows", None)
        self.assertEqual(environment.get("PYTHONIOENCODING"), "utf-8")

    def test_host_command_wraps_batch_files_on_windows(self):
        """CreateProcess は .cmd/.bat を直接実行できないので cmd /c を挟む。"""
        expected = ["cmd", "/c", "npm.cmd", "ci"] if os.name == "nt" else ["npm.cmd", "ci"]
        self.assertEqual(run.host_command(["npm.cmd", "ci"]), expected)
        self.assertEqual(run.host_command(["cargo", "test"]), ["cargo", "test"])

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

    def test_local_os_outside_runs_on_is_reported_but_still_runs(self):
        """CI が別 OS でも、手元は手元の OS の step を実行する（ふりをしない）。"""
        route = route_for_os("windows")
        with patch.object(run.report, "on_github", return_value=False), \
             patch.object(run, "local_os", return_value="linux"), \
             patch.object(run, "route_for", return_value=route), \
             patch.object(run, "missing_prerequisites", return_value=[]), \
             patch.object(run, "execute", return_value={"route": "test", "ok": True, "steps": []}) as execute:
            with redirect_stderr(io.StringIO()) as errors:
                self.assertEqual(invoke(["test"]), 0)
        execute.assert_called_once()
        self.assertIn("runs on windows in CI", errors.getvalue())

    def test_os_flag_runs_the_given_os_variant(self):
        """`--os` は CI の matrix 用。ローカルでも OS を明示できる。"""
        route = route_for_os("windows")
        with patch.object(run.report, "on_github", return_value=False), \
             patch.object(run, "local_os", return_value="linux"), \
             patch.object(run, "route_for", return_value=route), \
             patch.object(run, "missing_prerequisites", return_value=[]), \
             patch.object(run, "execute", return_value={"route": "test", "ok": True, "steps": []}) as execute:
            with redirect_stdout(io.StringIO()):
                self.assertEqual(invoke(["test", "--os", "windows"]), 0)
        self.assertEqual(execute.call_args.args[3], "windows")

    def test_os_flag_rejects_unknown_os(self):
        with redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit):
                invoke(["test", "--os", "solaris"])

    def test_skipped_steps_count_as_processed(self):
        """前提不足で skip した step も進捗に数える（1/2 のまま止まって見えない）。"""
        with tempfile.TemporaryDirectory() as directory:
            route = route_with(Step("needs-gradle", ["gradle", "build"], requires=("gradle",)))
            # gradle は GitHub のランナーに最初から入っているため、実環境の有無に
            # 頼ると skip されない。前提判定は別テストが担うのでここでは固定する。
            with patch.object(report, "on_github", return_value=False), \
                 patch.object(run, "missing_prerequisites_for_step", return_value=["gradle"]), \
                 patch.dict(os.environ, {state.ENV: directory}):
                payload = run.execute(route, None, quiet=True)
                payload_state = state.read(state.path_for(Path(directory), "test"))
        self.assertTrue(payload["ok"])
        self.assertTrue(payload["steps"][0]["skipped"])
        self.assertEqual(payload_state["steps_done"], 1)

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

            def fake_step(_route, step, _log, _writer=None, _os_name=None, _temp=None):
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


class StepPathsTests(unittest.TestCase):
    """`Step.paths` で変更 path に無関係な step を skip する。"""

    def test_step_without_paths_always_runs(self):
        step = Step("always", [sys.executable, "-c", "pass"])
        self.assertTrue(run.step_paths_match(step, []))
        self.assertTrue(run.step_paths_match(step, ["host/app/src/client.rs"]))
        self.assertTrue(run.step_paths_match(step, None))

    def test_step_paths_skip_when_no_changed_path_matches(self):
        step = Step(
            "web", [sys.executable, "-c", "pass"],
            paths=("host/transport/web/", "host/app/src/web_publisher.rs"),
        )
        self.assertFalse(run.step_paths_match(step, ["host/app/src/client.rs"]))
        self.assertFalse(run.step_paths_match(step, []))

    def test_step_paths_run_when_a_changed_path_matches(self):
        step = Step(
            "web", [sys.executable, "-c", "pass"],
            paths=("host/transport/web/", "host/app/src/web_publisher.rs"),
        )
        self.assertTrue(run.step_paths_match(step, ["host/transport/web/src/main.rs"]))
        self.assertTrue(run.step_paths_match(step, ["host/app/src/web_publisher.rs"]))

    def test_unknown_base_runs_every_step(self):
        """base を解決できないときは安全側に倒して全 step を実行する。"""
        step = Step("web", [sys.executable, "-c", "pass"], paths=("host/transport/web/",))
        self.assertTrue(run.step_paths_match(step, None))

    def test_paths_skip_is_reported_as_a_skipped_step(self):
        route = route_with(
            Step("web", [sys.executable, "-c", "pass"], paths=("host/transport/web/",)),
            Step("always", [sys.executable, "-c", "pass"]),
        )

        def fake_step(_route, step, _log, _writer=None, _os_name=None, _temp=None):
            return {"name": step.name, "ok": True, "seconds": 0.0, "output": ""}

        with tempfile.TemporaryDirectory() as directory, \
             patch.object(report, "on_github", return_value=False), \
             patch.object(run, "run_step", side_effect=fake_step) as step_runner, \
             patch.object(report, "notice") as notice, \
             patch.dict(os.environ, {state.ENV: directory}):
            payload = run.execute(
                route, None, quiet=True, changed=["host/app/src/client.rs"]
            )

        self.assertTrue(payload["ok"])
        self.assertEqual(
            payload["steps"][0],
            {"name": "web", "ok": True, "skipped": True, "seconds": 0.0},
        )
        self.assertNotIn("skipped", payload["steps"][1])
        self.assertEqual([call.args[1].name for call in step_runner.call_args_list], ["always"])
        self.assertTrue(
            any("skip web (no matching path)" in str(call.args[0]) for call in notice.call_args_list)
        )

    def test_all_steps_run_when_changed_paths_are_unknown(self):
        route = route_with(
            Step("web", [sys.executable, "-c", "pass"], paths=("host/transport/web/",)),
        )

        def fake_step(_route, step, _log, _writer=None, _os_name=None, _temp=None):
            return {"name": step.name, "ok": True, "seconds": 0.0, "output": ""}

        with tempfile.TemporaryDirectory() as directory, \
             patch.object(report, "on_github", return_value=False), \
             patch.object(run, "run_step", side_effect=fake_step) as step_runner, \
             patch.dict(os.environ, {state.ENV: directory}):
            payload = run.execute(route, None, quiet=True, changed=None)

        self.assertTrue(payload["ok"])
        self.assertEqual(step_runner.call_count, 1)
        self.assertNotIn("skipped", payload["steps"][0])

    def test_select_changed_returns_none_without_a_base(self):
        with patch.object(run.paths, "changed_paths") as changed:
            self.assertIsNone(run.select_changed(None))
            self.assertIsNone(run.select_changed(""))
        changed.assert_not_called()

    def test_select_changed_uses_the_committed_diff_only(self):
        with patch.object(run.paths, "changed_paths", return_value=["a"]) as changed:
            self.assertEqual(run.select_changed("origin/develop"), ["a"])
        changed.assert_called_once_with("origin/develop", include_worktree=False)

    def test_main_passes_base_diff_to_execute(self):
        route = route_for_os("linux")
        with patch.object(run.report, "on_github", return_value=False), \
             patch.object(run, "local_os", return_value="linux"), \
             patch.object(run, "route_for", return_value=route), \
             patch.object(run, "missing_prerequisites", return_value=[]), \
             patch.object(run.paths, "changed_paths", return_value=["host/app/src/client.rs"]), \
             patch.object(run, "execute", return_value={"route": "test", "ok": True, "steps": []}) as execute:
            with redirect_stdout(io.StringIO()):
                self.assertEqual(invoke(["test", "--base", "origin/develop"]), 0)
        self.assertEqual(execute.call_args.kwargs["changed"], ["host/app/src/client.rs"])

    def test_main_defaults_base_to_base_sha_env(self):
        route = route_for_os("linux")
        with patch.object(run.report, "on_github", return_value=False), \
             patch.object(run, "local_os", return_value="linux"), \
             patch.object(run, "route_for", return_value=route), \
             patch.object(run, "missing_prerequisites", return_value=[]), \
             patch.object(run.paths, "changed_paths", return_value=None) as changed, \
             patch.object(run, "execute", return_value={"route": "test", "ok": True, "steps": []}) as execute, \
             patch.dict(os.environ, {"BASE_SHA": "abc123"}):
            with redirect_stdout(io.StringIO()):
                self.assertEqual(invoke(["test"]), 0)
        changed.assert_called_once_with("abc123", include_worktree=False)
        self.assertIsNone(execute.call_args.kwargs["changed"])


def cargo_message(level="warning", file="src/lib.rs", line=1, column=1):
    return json.dumps({
        "reason": "compiler-message",
        "message": {
            "level": level,
            "message": "unused variable",
            "code": {"code": "unused_variables"},
            "rendered": f"{level}: unused variable\n",
            "spans": [
                {
                    "file_name": file,
                    "line_start": line,
                    "column_start": column,
                    "line_end": line,
                    "column_end": column + 1,
                    "is_primary": True,
                }
            ],
        },
    })


def print_line(line: str) -> str:
    """1 行を標準出力へ書くだけの Python コマンド。"""
    return "import sys; sys.stdout.write(" + repr(line) + ")"


class JsonDiagnosticsTests(unittest.TestCase):
    def test_warning_fails_the_step_even_when_the_command_succeeds(self):
        route = route_with(Step(
            "lint",
            [sys.executable, "-c", print_line(cargo_message())],
            json_diagnostics=True,
        ))
        with tempfile.TemporaryDirectory() as directory:
            with patch.object(report, "on_github", return_value=False), \
                 patch.dict(os.environ, {state.ENV: directory}):
                payload = run.execute(route, None, quiet=True)
        self.assertFalse(payload["ok"])

    def test_clean_json_output_passes(self):
        artifact = json.dumps({"reason": "compiler-artifact", "target": {"name": "demo"}})
        route = route_with(Step(
            "lint",
            [sys.executable, "-c", print_line(artifact)],
            json_diagnostics=True,
        ))
        with tempfile.TemporaryDirectory() as directory:
            with patch.object(report, "on_github", return_value=False), \
                 patch.dict(os.environ, {state.ENV: directory}):
                payload = run.execute(route, None, quiet=True)
        self.assertTrue(payload["ok"])

    def test_diagnostics_are_annotated_on_github(self):
        route = route_with(Step(
            "lint",
            [sys.executable, "-c", print_line(cargo_message(file="clients/ui/src/root.rs", line=7, column=3))],
            json_diagnostics=True,
        ))
        with tempfile.TemporaryDirectory() as directory:
            with patch.object(report, "on_github", return_value=True), \
                 patch.dict(os.environ, {state.ENV: directory}):
                with redirect_stdout(io.StringIO()) as output:
                    payload = run.execute(route, None, quiet=True)
        self.assertIn(
            "::warning file=clients/ui/src/root.rs,line=7,col=3",
            output.getvalue(),
        )
        self.assertFalse(payload["ok"])


def route_for_os(name: str, **kwargs) -> Route:
    return Route(key="test", name="Test", runs_on=(name,), timeout_minutes=5,
                 steps=(Step("ok", [sys.executable, "-c", "pass"]),), **kwargs)


if __name__ == "__main__":
    unittest.main()
