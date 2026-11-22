"""依存ホワイトリスト検査のユニットテスト。"""

import json
from pathlib import Path
import tempfile
import unittest

from check_dependencies import check, load_allowlist, load_metadata


def package(name, dependencies=()):
    return {
        "name": name,
        "id": name,
        "manifest_path": f"/workspace/{name}/Cargo.toml",
        "dependencies": [{"name": dependency} for dependency in dependencies],
    }


def metadata(*packages):
    return {
        "workspace_members": [package["id"] for package in packages],
        "packages": list(packages),
    }


class DependencyAllowlistTests(unittest.TestCase):
    def test_listed_edge_is_allowed(self):
        tree = metadata(package("app", ["protocol"]), package("protocol"))
        self.assertEqual(check(tree, {"app": {"protocol"}}), [])

    def test_unlisted_edge_is_rejected(self):
        tree = metadata(package("app", ["host"]), package("host"))
        errors = check(tree, {"app": {"protocol"}})
        self.assertEqual(len(errors), 1)
        self.assertIn("app -> host", errors[0])
        self.assertIn("/workspace/app/Cargo.toml", errors[0])

    def test_removed_edge_does_not_fail(self):
        tree = metadata(package("app"), package("protocol"))
        self.assertEqual(check(tree, {"app": {"protocol"}}), [])

    def test_dependency_outside_workspace_is_ignored(self):
        tree = metadata(package("app", ["serde"]))
        self.assertEqual(check(tree, {}), [])

    def test_allowlist_rejects_invalid_shape(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "allowlist.json"
            path.write_text(json.dumps(["app"]), encoding="utf-8")
            with self.assertRaises(ValueError):
                load_allowlist(path)
            path.write_text(json.dumps({"app": "protocol"}), encoding="utf-8")
            with self.assertRaises(ValueError):
                load_allowlist(path)
            path.write_text(json.dumps({"app": [1]}), encoding="utf-8")
            with self.assertRaises(ValueError):
                load_allowlist(path)

    def test_current_workspace_matches_allowlist(self):
        self.assertEqual(check(load_metadata(), load_allowlist()), [])


if __name__ == "__main__":
    unittest.main()
