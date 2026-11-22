"""cargo の JSON 診断の解釈（scripts/ci/diagnostics.py）のテスト。"""

import json
import unittest

from scripts.ci.diagnostics import Collector


def message(
    level="warning",
    code="unused_variables",
    file="src/lib.rs",
    line=1,
    column=1,
    text="unused variable",
    rendered=None,
):
    return json.dumps({
        "reason": "compiler-message",
        "message": {
            "level": level,
            "message": text,
            "code": {"code": code} if code else None,
            "rendered": rendered if rendered is not None else f"{level}: {text}\n",
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


class CollectorTests(unittest.TestCase):
    def test_partial_lines_are_buffered_until_complete(self):
        line = message()
        collector = Collector()
        self.assertEqual(collector.feed(line[:20]), "")
        self.assertEqual(collector.feed(line[20:] + "\n"), "warning: unused variable\n")
        self.assertEqual(len(collector.items), 1)

    def test_warning_is_a_failure_even_if_cargo_succeeds(self):
        collector = Collector()
        collector.feed(message(level="warning") + "\n")
        self.assertTrue(collector.failed())

    def test_clean_build_is_not_a_failure(self):
        collector = Collector()
        collector.feed(json.dumps({"reason": "compiler-artifact", "target": {"name": "demo"}}) + "\n")
        self.assertFalse(collector.failed())
        self.assertIn("Compiling demo", collector.text)

    def test_duplicate_diagnostics_are_counted_once(self):
        """--all-targets では同じ診断が lib と test で二重に出る。"""
        collector = Collector()
        collector.feed(message() + "\n" + message() + "\n")
        self.assertEqual(len(collector.items), 1)

    def test_annotations_carry_file_line_and_rule(self):
        collector = Collector()
        collector.feed(message(
            code="clippy::disallowed_methods",
            file="clients/ui/src/root.rs",
            line=12,
            column=5,
        ) + "\n")
        annotation = collector.annotations()[0]
        self.assertTrue(annotation.startswith("::warning "))
        self.assertIn("file=clients/ui/src/root.rs,line=12,col=5", annotation)
        self.assertIn("title=clippy%3A%3Adisallowed_methods", annotation)

    def test_diagnostics_without_a_span_have_no_file(self):
        line = json.dumps({
            "reason": "compiler-message",
            "message": {
                "level": "warning",
                "message": "multiple crate versions",
                "code": {"code": "clippy::multiple_crate_versions"},
                "rendered": "warning: multiple crate versions\n",
                "spans": [],
            },
        })
        collector = Collector()
        collector.feed(line + "\n")
        self.assertEqual(
            collector.annotations()[0],
            "::warning::clippy::multiple_crate_versions: multiple crate versions",
        )

    def test_non_json_lines_are_shown_as_is(self):
        collector = Collector()
        self.assertEqual(
            collector.feed("error: could not find Cargo.toml\n"),
            "error: could not find Cargo.toml\n",
        )

    def test_summary_groups_by_rule_and_file(self):
        collector = Collector()
        collector.feed(message(code="clippy::a", file="a.rs") + "\n")
        collector.feed(message(code="clippy::a", file="a.rs", line=2) + "\n")
        collector.feed(message(code="clippy::b", file="b.rs") + "\n")
        summary = collector.summary()
        self.assertIn("- warnings: 3", summary)
        self.assertIn("| `clippy::a` | 2 |", summary)
        self.assertIn("| `a.rs` | 2 |", summary)

    def test_flush_processes_the_last_incomplete_line(self):
        collector = Collector()
        self.assertEqual(collector.feed("error: no newline at end"), "")
        self.assertEqual(collector.flush(), "error: no newline at end\n")
        self.assertEqual(collector.flush(), "")


if __name__ == "__main__":
    unittest.main()
