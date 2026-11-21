"""`python -m scripts.ci.run` の CLI と一覧表示を確認する。

実際の toolchain に依存しない範囲（引数解釈・一覧・前提表示）だけを見る。
"""

import io
import sys
import unittest
from contextlib import redirect_stderr, redirect_stdout
from unittest.mock import patch

from scripts.ci import run
from scripts.ci.jobs import ORDER


def invoke(argv) -> int:
    with patch.object(sys, "argv", ["run", *argv]):
        return run.main()


class ListTests(unittest.TestCase):
    def test_list_shows_every_route_and_prerequisites(self):
        with redirect_stdout(io.StringIO()) as buffer:
            self.assertEqual(invoke(["--list"]), 0)
        output = buffer.getvalue()
        for key in ORDER:
            self.assertIn(key, output)
        self.assertIn("requires", output)

    def test_no_arguments_prints_usage_and_fails(self):
        with redirect_stdout(io.StringIO()) as buffer:
            self.assertEqual(invoke([]), 2)
        self.assertIn("--list", buffer.getvalue())

    def test_unknown_route_is_rejected(self):
        # route が未知なら SystemExit(2) で終わる。
        with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()) as errors:
            with self.assertRaises(SystemExit) as raised:
                invoke(["nope"])
        self.assertEqual(raised.exception.code, 2)
        self.assertIn("known routes", errors.getvalue())

    def test_bad_step_name_lists_available_steps(self):
        with redirect_stdout(io.StringIO()), self.assertRaises(SystemExit) as raised:
            invoke(["repository", "--step", "Nope"])
        self.assertIn("available", str(raised.exception.code))


if __name__ == "__main__":
    unittest.main()
