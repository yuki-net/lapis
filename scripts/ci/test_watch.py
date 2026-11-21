"""`scripts/ci/watch.py` の表示と成否判定を、プロセスを起動せずに確認する。"""

import os
import subprocess
import sys
import unittest

from scripts.ci import plan, state, watch


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

    def test_running_line_shows_time_since_last_output(self):
        """止まっていないかを判断できるよう、無出力の経過を出す。"""
        found = running_state("host", last_output_at=180.0)
        line = watch.line_for(entry("host"), found, now=200.0, palette=NO_COLOR)
        self.assertIn("no output 20s", line)

    def test_pending_line_shows_total_steps_before_it_starts(self):
        target = entry("host")
        states = {}
        found = watch.status_of(target, states, codes={})
        line = watch.line_for(target, found, now=200.0, palette=NO_COLOR)
        self.assertIn(f"0/{target['steps']}", line)
        self.assertIn("(waiting)", line)

    def test_failed_line_names_the_failed_step(self):
        found = running_state("host", status=state.FAILED, failed_step="Test Host")
        line = watch.line_for(entry("host"), found, now=200.0, palette=NO_COLOR)
        self.assertIn("Test Host", line)


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

    def test_finished_process_without_final_state_is_reported_by_exit_code(self):
        """状態を書けずに落ちても、実行中のまま見せない。"""
        target = entry("host")
        states = {"host": running_state("host")}
        found = watch.status_of(target, states, codes={"host": 1})
        self.assertEqual(found["status"], state.FAILED)

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
    def test_entries_come_from_the_plan_matrix(self):
        keys = plan.route_keys(["clients/web/package.json"])
        entries = plan.matrix(keys)
        self.assertEqual([found["job"] for found in entries], ["web"])

    def test_os_filter_keeps_only_that_os(self):
        entries = plan.matrix(list(plan.ORDER), only_os="windows")
        self.assertTrue(entries)
        self.assertEqual({found["os"] for found in entries}, {"windows"})


if __name__ == "__main__":
    unittest.main()
