import copy
from pathlib import Path
import unittest

from check_architecture import check

ROOT = Path(__file__).resolve().parents[1]


def package(name, path, dependencies=()):
    return {"name": name, "id": name, "manifest_path": str(ROOT / path / "Cargo.toml"),
            "dependencies": [{"name": dependency} for dependency in dependencies]}


class ArchitectureTests(unittest.TestCase):
    def setUp(self):
        self.metadata = {"workspace_members": ["model", "runtime", "ui"], "packages": [
            package("model", "shared/models/example"),
            package("runtime", "backend/runtime", ["model"]),
            package("ui", "apps/desktop/ui", ["model"]),
        ]}

    def test_allowed_dependencies(self):
        self.assertEqual(check(self.metadata, {}), [])

    def test_reverse_dependency_fails(self):
        self.metadata["packages"][0]["dependencies"] = [{"name": "ui", "kind": "dev", "target": "cfg(windows)"}]
        self.assertTrue(check(self.metadata, {}))

    def test_exact_exception_does_not_allow_another_edge(self):
        self.metadata["packages"][0]["dependencies"] = [{"name": "ui"}, {"name": "runtime"}]
        errors = check(self.metadata, {"model -> ui": "migration"})
        self.assertEqual(len(errors), 1)
        self.assertIn("model -> runtime", errors[0])

    def test_stale_and_empty_exceptions_fail(self):
        self.assertTrue(check(self.metadata, {"ui -> model": "stale"}))
        self.metadata["packages"][0]["dependencies"] = [{"name": "ui"}]
        self.assertTrue(check(self.metadata, {"model -> ui": ""}))

    def test_unknown_location_fails(self):
        self.metadata["packages"][0]["manifest_path"] = str(ROOT / "features/unknown/Cargo.toml")
        self.assertTrue(check(self.metadata, {}))

    def test_backend_platform_dependency_fails(self):
        metadata = copy.deepcopy(self.metadata)
        metadata["packages"][1]["dependencies"].append({"name": "gpui"})
        self.assertTrue(check(metadata, {}))


if __name__ == "__main__":
    unittest.main()
