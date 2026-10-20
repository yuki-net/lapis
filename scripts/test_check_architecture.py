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

    def test_regular_client_cannot_depend_on_backend_implementation(self):
        self.metadata["workspace_members"].extend(["cli", "infra"])
        self.metadata["packages"].extend([
            package("cli", "apps/cli", ["infra"]),
            package("infra", "backend/infrastructure/local"),
        ])
        self.assertIn("cli -> infra", "\n".join(check(self.metadata, {})))

    def test_regular_client_can_depend_on_protocol(self):
        self.metadata["workspace_members"].extend(["cli", "protocol"])
        self.metadata["packages"].extend([
            package("cli", "apps/cli", ["protocol"]),
            package("protocol", "shared/protocol"),
        ])
        self.assertEqual(check(self.metadata, {}), [])

    def test_desktop_entry_can_compose_backend(self):
        self.metadata["workspace_members"].extend(["desktop", "infra"])
        self.metadata["packages"].extend([
            package("desktop", "apps/desktop/app", ["ui", "runtime", "infra"]),
            package("infra", "backend/infrastructure/local"),
        ])
        self.assertEqual(check(self.metadata, {}), [])

    def test_desktop_ui_cannot_depend_on_infrastructure(self):
        self.metadata["workspace_members"].append("infra")
        self.metadata["packages"].append(package("infra", "backend/infrastructure/local"))
        self.metadata["packages"][2]["dependencies"].append({"name": "infra"})
        self.assertIn("ui -> infra", "\n".join(check(self.metadata, {})))

    def test_only_desktop_composition_root_can_depend_on_host(self):
        self.metadata["workspace_members"].extend(["desktop", "cli", "host"])
        self.metadata["packages"].extend([
            package("desktop", "apps/desktop/app", ["host"]),
            package("cli", "apps/cli"),
            package("host", "backend/host", ["runtime"]),
        ])
        self.assertEqual(check(self.metadata, {}), [])
        for client in ("ui", "cli"):
            with self.subTest(client=client):
                metadata = copy.deepcopy(self.metadata)
                target = next(item for item in metadata["packages"] if item["name"] == client)
                target["dependencies"].append({"name": "host"})
                errors = check(metadata, {})
                self.assertEqual(len(errors), 1)
                self.assertIn(f"{client} -> host", errors[0])

    def test_backend_host_cannot_depend_on_gpui_or_rfd(self):
        for dependency in ("gpui", "rfd"):
            with self.subTest(dependency=dependency):
                metadata = copy.deepcopy(self.metadata)
                metadata["workspace_members"].append("host")
                metadata["packages"].append(
                    package("host", "backend/host", [dependency])
                )
                self.assertIn(
                    f"host -> {dependency}: Client 固有依存",
                    check(metadata, {}),
                )


if __name__ == "__main__":
    unittest.main()
