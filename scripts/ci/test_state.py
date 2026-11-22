"""`scripts/ci/state.py` の記録形式を確認する。"""

import json
import tempfile
import unittest
import unittest.mock
from pathlib import Path

from scripts.ci import state


class StateTests(unittest.TestCase):
    def test_write_and_read_round_trip(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            state.write(state.path_for(base, "host"), {"job": "host", "status": state.RUNNING})
            self.assertEqual(state.read(state.path_for(base, "host"))["status"], state.RUNNING)

    def test_read_all_ignores_broken_files(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            state.write(state.path_for(base, "host"), {"job": "host", "status": state.PASSED})
            (base / "broken.json").write_text("{not json", encoding="utf-8")
            self.assertEqual(sorted(state.read_all(base)), ["host"])

    def test_writer_records_steps_and_finish(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            writer = state.Writer(base, "host", "Host / Windows", "windows", 2)
            self.assertEqual(state.read(state.path_for(base, "host"))["status"], state.RUNNING)

            writer.begin("Check formatting")
            writer.end("Check formatting", True)
            writer.begin("Test Host")
            writer.end("Test Host", False)
            writer.finish(False)

            payload = state.read(state.path_for(base, "host"))
        self.assertEqual(payload["steps_done"], 2)
        self.assertEqual(payload["failed_step"], "Test Host")
        self.assertEqual(payload["status"], state.FAILED)
        self.assertIsNotNone(payload["finished_at"])
        self.assertIsNone(payload["current_step"])

    def test_writer_without_directory_is_a_no_op(self):
        writer = state.Writer(None, "host", "Host / Windows", "windows", 1)
        writer.begin("x")
        writer.end("x", True)
        writer.finish(True)
        self.assertEqual(writer.data["status"], state.PASSED)

    def test_directory_follows_the_environment(self):
        with tempfile.TemporaryDirectory() as directory:
            with unittest.mock.patch.dict("os.environ", {state.ENV: directory}):
                self.assertEqual(state.directory(), Path(directory))
        with unittest.mock.patch.dict("os.environ", {}, clear=True):
            self.assertIsNone(state.directory())


if __name__ == "__main__":
    unittest.main()
