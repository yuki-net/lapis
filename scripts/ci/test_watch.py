"""`scripts/ci/watch.py` の表示と成否判定を、プロセスを起動せずに確認する。"""

import io
import os
import subprocess
import sys
import unittest
from unittest.mock import patch

from scripts.ci import model, plan, state, watch


NO_COLOR = watch.Palette(False)


def entry(job: str) -> dict:
    return plan.matrix([job])[0]


def running_state(job: str, **overrides) -> dict:
    payload = {
        "job": job,
        "name": entry(job)["name"],
        "status": state.RUNNING,
        "steps_total": 4,
        "steps_done": 1,
        "current_step": "Test Host",
        "started_at": 100.0,
        "finished_at": None,
        "last_output_at": None,
        "failed_step": None,
    }
    payload.update(overrides)
    return payload


class ProgressBarTests(unittest.TestCase):
    def test_bar_reflects_done_ratio(self):
        self.assertEqual(watch.progress_bar(0, 4), "-" * watch.BAR_WIDTH)
        self.assertEqual(watch.progress_bar(4, 4), "#" * watch.BAR_WIDTH)
        self.assertEqual(watch.progress_bar(2, 4).count("#"), watch.BAR_WIDTH // 2)

    def test_bar_without_total_is_unknown(self):
        self.assertEqual(watch.progress_bar(0, 0), "-" * watch.BAR_WIDTH)


class LineTests(unittest.TestCase):
    def test_each_status_has_its_own_mark(self):
        expected = {
            state.PENDING: "..",
            state.RUNNING: ">>",
            state.PASSED: "OK",
            state.FAILED: "NG",
            state.CANCELLED: "--",
            state.SKIPPED: "--",
        }
        for status, mark in expected.items():
            found = running_state("host", status=status)
            line = watch.line_for(entry("host"), found, now=200.0, palette=NO_COLOR)
            self.assertIn(mark, line, status)

    def test_running_line_shows_the_file_being_processed(self):
        """出力が変わると分かるよう、今処理しているファイルを出す。"""
        found = running_state("host", last_line="   Compiling host v0.1.0 (host/app)")
        line = watch.line_for(entry("host"), found, now=200.0, palette=NO_COLOR)
        self.assertIn("host/app", line)

    def test_activity_prefers_a_file_path_over_the_raw_line(self):
        self.assertEqual(
            watch.activity_of({"last_line": "warning: unused import in clients/ui/src/shell/layout.rs:12"}),
            "clients/ui/src/shell/layout.rs",
        )
        self.assertEqual(
            watch.activity_of({"last_line": "test result: ok. 12 passed"}),
            "test result: ok. 12 passed",
        )
        self.assertEqual(watch.activity_of({}), "")

    def test_left_name_is_the_github_job_name(self):
        """左は GitHub に出る job 名（Mobile / Android / Windows など）。"""
        target = plan.local_entries(["mobile"])[0]
        line = watch.line_for(target, running_state("mobile", status=state.PASSED), now=200.0, palette=NO_COLOR)
        self.assertIn(target["name"], line)
        self.assertTrue(target["name"].startswith("Mobile / Android"))

    def test_pending_line_shows_total_steps_before_it_starts(self):
        target = entry("host")
        states = {}
        found = watch.status_of(target, states, codes={})
        line = watch.line_for(target, found, now=200.0, palette=NO_COLOR)
        self.assertIn(f"0/{target['steps']}", line)
        self.assertIn("(waiting)", line)

    def test_failed_line_leaves_the_reason_to_the_end_summary(self):
        """行には理由を出さない（終了後にまとめて出す）。"""
        found = running_state("host", status=state.FAILED, failed_step="Test Host")
        line = watch.line_for(entry("host"), found, now=200.0, palette=NO_COLOR)
        self.assertNotIn("Test Host", line)
        self.assertIn("NG", line)


class ScreenTests(unittest.TestCase):
    def test_truncate_counts_visible_width_not_escape_sequences(self):
        coloured = f"{watch.GREEN}OK{watch.RESET} Host / Windows"
        self.assertEqual(watch.visible_length(coloured), len("OK Host / Windows"))
        cut = watch.truncate_ansi(coloured, 5)
        self.assertIn(watch.GREEN, cut)
        self.assertEqual(watch.visible_length(cut), 5)

    def test_truncate_leaves_short_lines_alone(self):
        self.assertEqual(watch.truncate_ansi("OK Host", 20), "OK Host")


    def test_commit_writes_a_permanent_line_without_ansi(self):
        buffer = io.StringIO()
        watch.Screen(buffer, ansi=False).commit("OK Host")
        self.assertEqual(buffer.getvalue(), "OK Host\n")

    def test_frame_is_a_no_op_without_ansi(self):
        buffer = io.StringIO()
        watch.Screen(buffer, ansi=False).frame(["a", "b"])
        self.assertEqual(buffer.getvalue(), "")

    def test_frame_switches_to_the_alternate_screen_once(self):
        buffer = io.StringIO()
        screen = watch.Screen(buffer, ansi=True)
        with patch.object(watch.shutil, "get_terminal_size", return_value=os.terminal_size((80, 24))):
            screen.frame(["a"])
            screen.frame(["b"])
        output = buffer.getvalue()
        self.assertEqual(output.count(watch.ENTER_ALT_SCREEN), 1)
        self.assertIn(watch.HIDE_CURSOR, output)

    def test_fit_lines_truncates_width_and_height(self):
        fitted = watch.fit_lines(["x" * 20, "y", "z"], width=5, height=2)
        self.assertEqual(watch.visible_length(fitted[0]), 5)
        self.assertIn("more line", fitted[-1])

    def test_close_leaves_the_final_lines_on_the_normal_screen(self):
        buffer = io.StringIO()
        screen = watch.Screen(buffer, ansi=True)
        with patch.object(watch.shutil, "get_terminal_size", return_value=os.terminal_size((80, 24))):
            screen.frame(["a"])
        screen.close(["1 passed"])
        output = buffer.getvalue()
        self.assertIn(watch.LEAVE_ALT_SCREEN, output)
        self.assertTrue(output.endswith("1 passed\n"))


class SummaryTests(unittest.TestCase):
    def test_exit_code_is_zero_only_when_every_job_passed(self):
        entries = [entry("host"), entry("web")]
        states = {
            "host": {"job": "host", "status": state.PASSED},
            "web": {"job": "web", "status": state.PASSED},
        }
        self.assertEqual(watch.exit_code(entries, states), 0)

        states["web"] = {"job": "web", "status": state.FAILED}
        self.assertEqual(watch.exit_code(entries, states), 1)

        states["web"] = {"job": "web", "status": state.CANCELLED}
        self.assertEqual(watch.exit_code(entries, states), 1)

    def test_summary_lists_passed_and_failed(self):
        entries = [entry("host"), entry("web")]
        states = {
            "host": {"job": "host", "status": state.PASSED},
            "web": {"job": "web", "status": state.FAILED},
        }
        text = "\n".join(watch.summary_lines(entries, states, NO_COLOR))
        self.assertIn("1 passed", text)
        self.assertIn("1 failed", text)

    def test_failure_lines_explain_why_each_job_failed(self):
        """終了後に、どの job がどの step で何を出して落ちたかを出す。"""
        entries = [entry("host"), entry("web")]
        states = {
            "host": {
                "job": "host",
                "name": "Host / Windows",
                "status": state.FAILED,
                "failed_step": "Test Host",
                "failure": "error[E0433]: failed to resolve",
            },
            "web": {"job": "web", "name": "Web / Linux", "status": state.PASSED},
        }
        text = "\n".join(watch.failure_lines(entries, states, NO_COLOR))
        self.assertIn("Host / Windows", text)
        self.assertIn("Test Host", text)
        self.assertIn("error[E0433]", text)
        self.assertNotIn("Web / Linux", text)

    def test_failure_lines_are_empty_when_nothing_failed(self):
        entries = [entry("host")]
        states = {"host": {"job": "host", "status": state.PASSED}}
        self.assertEqual(watch.failure_lines(entries, states, NO_COLOR), [])

    def test_finished_process_without_final_state_is_reported_by_exit_code(self):
        """状態を書けずに落ちても、実行中のまま見せない。"""
        target = entry("host")
        states = {"host": running_state("host")}
        found = watch.status_of(target, states, codes={"host": 1})
        self.assertEqual(found["status"], state.FAILED)
        self.assertIn("code 1", found["failure"])

    def test_missing_state_is_pending(self):
        self.assertEqual(watch.status_of(entry("web"), {}, codes={})["status"], state.PENDING)


class TerminateTests(unittest.TestCase):
    def test_terminate_stops_a_running_job(self):
        """Ctrl+C と同じ経路で、起動した job を確実に止められる。"""
        process = subprocess.Popen(
            [sys.executable, "-c", "import time; time.sleep(60)"],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=(os.name != "nt"),
            creationflags=(subprocess.CREATE_NEW_PROCESS_GROUP if os.name == "nt" else 0),
        )
        watch.terminate(process)
        process.wait(timeout=20)
        self.assertIsNotNone(process.returncode)

    def test_terminate_is_safe_after_the_job_already_exited(self):
        process = subprocess.Popen(
            [sys.executable, "-c", "pass"],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        process.wait(timeout=20)
        watch.terminate(process)


class SelectionTests(unittest.TestCase):
    def test_entries_come_from_the_local_os(self):
        """手元は 1 台なので、route ごとに 1 つの job（この機械の OS）になる。"""
        entries = plan.local_entries(["web"])
        self.assertEqual(len(entries), 1)
        self.assertEqual(entries[0]["job"], "web")
        self.assertEqual(entries[0]["os"], model.local_os())

    def test_os_filter_keeps_only_that_os(self):
        entries = plan.matrix(list(plan.ORDER), only_os="windows")
        self.assertTrue(entries)
        self.assertEqual({found["os"] for found in entries}, {"windows"})


if __name__ == "__main__":
    unittest.main()
