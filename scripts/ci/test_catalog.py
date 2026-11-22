"""`lints/` のカタログ・実装・テスト・fixture の三者一致を検査する。

カタログだけ足して実装が無い、実装だけ足してカタログが無い、回答が空、
fail fixture が実際には落ちない——といった「静かに死ぬルール」を防ぐ。
"""

import importlib
import unittest
from pathlib import Path

from scripts.check_architecture import check_file_length, file_length_errors
from scripts.ci import policy
from scripts.ci.model import ROOT

CATALOG = ROOT / "lints/catalog"
FIXTURES = ROOT / "lints/fixtures"
README = ROOT / "lints/README.md"

REQUIRED = ("id", "remedy", "scope", "implementation", "test", "added")
REMEDIES = {"design", "static", "test", "schema", "policy", "prose"}


def front_matter(path):
    """markdown 先頭の front matter を dict にする（YAML 依存を足さない）。"""
    text = path.read_text(encoding="utf-8").replace("\r\n", "\n")
    if not text.startswith("---\n"):
        raise ValueError(f"{path.name}: front matter が無い")
    header, separator, body = text[4:].partition("\n---\n")
    if not separator:
        raise ValueError(f"{path.name}: front matter が閉じていない")
    fields = {}
    for line in header.splitlines():
        if not line.strip():
            continue
        key, found, value = line.partition(":")
        if not found:
            raise ValueError(f"{path.name}: '{line}' は key: value ではない")
        fields[key.strip()] = value.strip()
    return fields, body


def catalog_entries():
    return {path.stem: front_matter(path) for path in sorted(CATALOG.glob("*.md"))}


def resolve(reference):
    """`module:qualname` を実体に解決する。"""
    module_name, separator, qualname = reference.partition(":")
    if not separator or not qualname:
        raise ValueError(f"'{reference}' は module:qualname の形で書く")
    value = importlib.import_module(module_name)
    for part in qualname.split("."):
        value = getattr(value, part)
    return value


def read_paths(path):
    return [line for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]


class CatalogTests(unittest.TestCase):
    def test_front_matter_has_required_fields(self):
        for name, (fields, body) in catalog_entries().items():
            for key in REQUIRED:
                self.assertTrue(fields.get(key), f"{name}: {key} が空")
            self.assertIn(fields["remedy"], REMEDIES, name)
            self.assertTrue(body.strip(), f"{name}: 本文が空")
            for heading in ("## 何を検知するか", "## 直し方", "## 例外"):
                self.assertIn(heading, body, f"{name}: {heading} が無い")

    def test_ids_are_unique_and_match_file_names(self):
        for name, (fields, _) in catalog_entries().items():
            self.assertEqual(fields["id"], name)

    def test_readme_lists_every_rule(self):
        readme = README.read_text(encoding="utf-8")
        for name in catalog_entries():
            self.assertIn(f"catalog/{name}.md", readme, f"README に {name} が無い")

    def test_implementation_and_test_exist(self):
        for name, (fields, _) in catalog_entries().items():
            for key in ("implementation", "test"):
                self.assertIsNotNone(resolve(fields[key]), f"{name}: {key}")

    def test_non_design_rules_explain_why_not_design(self):
        for name, (fields, _) in catalog_entries().items():
            if fields["remedy"] != "design":
                self.assertTrue(fields.get("why_not_design"), f"{name}: why_not_design が空")

    def test_fixtures_have_fail_and_pass(self):
        for name, (fields, _) in catalog_entries().items():
            if fields["remedy"] == "prose":
                continue
            for kind in ("fail", "pass"):
                directory = FIXTURES / name / kind
                self.assertTrue(directory.is_dir(), f"{name}: {kind} fixture が無い")
                self.assertTrue(any(directory.iterdir()), f"{name}: {kind} fixture が空")

    def test_file_length_fixtures(self):
        """上限を下げた fixture で、落ちる例と通る例を実際に走らせる。"""
        for kind, expected in (("fail", True), ("pass", False)):
            root = FIXTURES / "file-length" / kind
            sizes = {
                path.relative_to(root).as_posix(): len(path.read_text(encoding="utf-8").splitlines())
                for path in root.rglob("*.rs")
            }
            errors = file_length_errors(sizes, {}, 3)
            self.assertEqual(bool(errors), expected, f"{kind}: {errors}")

    def test_protected_paths_fixtures(self):
        fail = read_paths(FIXTURES / "protected-paths/fail/paths.txt")
        passing = read_paths(FIXTURES / "protected-paths/pass/paths.txt")
        self.assertEqual(len(policy.protected_hits(fail)), len(fail))
        self.assertEqual(policy.protected_hits(passing), [])

    def test_baseline_is_current(self):
        self.assertEqual(check_file_length(), [])


if __name__ == "__main__":
    unittest.main()
