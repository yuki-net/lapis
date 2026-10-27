import copy
import re
import tempfile
from pathlib import Path
import unittest

from check_architecture import check, check_desktop_layout

ROOT = Path(__file__).resolve().parents[1]


def package(name, path, dependencies=()):
    return {"name": name, "id": name, "manifest_path": str(ROOT / path / "Cargo.toml"),
            "dependencies": [{"name": dependency} for dependency in dependencies]}


class ArchitectureTests(unittest.TestCase):
    def test_desktop_layout_contract_rejects_old_identifiers_and_feature_internals(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "apps/desktop/ui/src/features/example.rs"
            source.parent.mkdir(parents=True)
            source.write_text('// TabGroupId was removed\nconst TEXT: &str = "DropZone";\n', encoding="utf-8")
            self.assertEqual(check_desktop_layout(root), [])
            source.write_text('fn use_old(_: TabGroupId) {}\n', encoding="utf-8")
            self.assertTrue(any("TabGroupId" in error for error in check_desktop_layout(root)))
            source.write_text('use crate::shell::panel_layout::PanelLayout;\n', encoding="utf-8")
            self.assertTrue(any("Feature" in error for error in check_desktop_layout(root)))

    def test_zone_persistence_rejects_legacy_names_and_helpers(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "apps/desktop/ui/src/shell/layout.rs"
            source.parent.mkdir(parents=True)
            source.write_text('fn panel_position() {}\nconst ZONE: &str = "left-panel";', encoding="utf-8")
            errors = check_desktop_layout(root)
            self.assertTrue(any("panel_position" in error for error in errors))
            self.assertTrue(any("left-panel" in error for error in errors))
            source.write_text('fn zone_position() {}\nconst ZONE: &str = "left";', encoding="utf-8")
            self.assertEqual(check_desktop_layout(root), [])

    def test_removed_zone_api_names_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "apps/desktop/ui/src/shell/example.rs"
            source.parent.mkdir(parents=True)
            for name in ("open_panel", "close_panel", "request_panel_open_with_duration", "ToggleBottomPanel", "panel_contributions", "set_panel_active_view"):
                source.write_text(f"fn {name}(zone: ZonePosition) {{}}", encoding="utf-8")
                self.assertTrue(any(name in error for error in check_desktop_layout(root)))
            source.write_text("fn close_panel(panel: PanelId) {} fn toggle_panel_visibility() {} fn set_panel_open() {}", encoding="utf-8")
            self.assertEqual(check_desktop_layout(root), [])
            source.write_text("struct PanelHost { position: ZonePosition }", encoding="utf-8")
            self.assertTrue(any("二重保持" in error for error in check_desktop_layout(root)))

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
        for path in ("backend/runtime", "backend/host"):
            for dependency in ("gpui", "rfd"):
                with self.subTest(path=path, dependency=dependency):
                    metadata = copy.deepcopy(self.metadata)
                    metadata["packages"][1]["manifest_path"] = str(ROOT / path / "Cargo.toml")
                    metadata["packages"][1]["dependencies"].append({
                        "name": dependency, "kind": "dev", "target": "cfg(windows)",
                    })
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

    def test_desktop_client_and_persistence_are_not_backend_or_ui(self):
        self.metadata["workspace_members"].extend(["desktop", "adapter", "store", "infra"])
        self.metadata["packages"].extend([
            package("desktop", "apps/desktop/app", ["ui", "adapter", "store"]),
            package("adapter", "apps/desktop/client", ["model"]),
            package("store", "apps/desktop/persistence", ["adapter", "model"]),
            package("infra", "backend/infrastructure/local", ["runtime"]),
        ])
        self.metadata["packages"][2]["dependencies"].append({"name": "adapter"})
        self.assertEqual(check(self.metadata, {}), [])
        for source, target in [("runtime", "adapter"), ("infra", "store"), ("adapter", "infra"),
                               ("adapter", "ui"), ("ui", "store"), ("store", "runtime")]:
            with self.subTest(source=source, target=target):
                metadata = copy.deepcopy(self.metadata)
                package_source = next(item for item in metadata["packages"] if item["name"] == source)
                package_source["dependencies"].append({"name": target, "kind": "dev", "target": "cfg(windows)"})
                self.assertIn(f"{source} -> {target}", "\n".join(check(metadata, {})))

    def test_client_adapter_and_store_cannot_import_gpui_or_native_dialog(self):
        for path in ("apps/desktop/client", "apps/desktop/persistence"):
            for dependency in ("gpui", "rfd"):
                with self.subTest(path=path, dependency=dependency):
                    metadata = copy.deepcopy(self.metadata)
                    metadata["workspace_members"].append("native")
                    metadata["packages"].append(package("native", path, [dependency]))
                    self.assertIn("Client 固有依存", "\n".join(check(metadata, {})))

    def test_comment_old_zone_api_is_allowed(self):
        """コメント内の旧Zone API署名はエラーにしない。"""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "apps/desktop/ui/src/shell/example.rs"
            source.parent.mkdir(parents=True)
            source.write_text('// fn close_panel(zone: ZonePosition) {}\n', encoding="utf-8")
            self.assertEqual(check_desktop_layout(root), [])

    def test_real_old_zone_api_is_rejected(self):
        """実コードの旧Zone API署名はエラーにする。"""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "apps/desktop/ui/src/shell/example.rs"
            source.parent.mkdir(parents=True)
            source.write_text('fn close_panel(zone: ZonePosition) {}', encoding="utf-8")
            self.assertTrue(any("close_panel" in error for error in check_desktop_layout(root)))

    def test_comment_panelhost_position_is_allowed(self):
        """コメント内のPanelHost.positionはエラーにしない。"""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "apps/desktop/ui/src/shell/example.rs"
            source.parent.mkdir(parents=True)
            source.write_text('// struct PanelHost { position: ZonePosition }', encoding="utf-8")
            self.assertEqual(check_desktop_layout(root), [])

    def test_real_panelhost_position_is_rejected(self):
        """実コードのPanelHost.positionはエラーにする。"""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "apps/desktop/ui/src/shell/example.rs"
            source.parent.mkdir(parents=True)
            source.write_text('struct PanelHost { position: ZonePosition }', encoding="utf-8")
            self.assertTrue(any("二重保持" in error for error in check_desktop_layout(root)))

    def test_comment_old_zone_save_name_is_allowed(self):
        """コメント内の旧Zone保存名はエラーにしない。"""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "apps/desktop/ui/src/shell/layout.rs"
            source.parent.mkdir(parents=True)
            source.write_text('// legacy value: "left-panel"', encoding="utf-8")
            self.assertEqual(check_desktop_layout(root), [])

    def test_real_old_zone_save_name_is_rejected(self):
        """実文字列の旧Zone保存名はエラーにする。"""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "apps/desktop/ui/src/shell/layout.rs"
            source.parent.mkdir(parents=True)
            source.write_text('const VALUE: &str = "left-panel";', encoding="utf-8")
            self.assertTrue(any("left-panel" in error for error in check_desktop_layout(root)))

    def test_block_comment_old_zone_api_is_allowed(self):
        """ブロックコメント内の旧Zone API署名もエラーにしない。"""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "apps/desktop/ui/src/shell/example.rs"
            source.parent.mkdir(parents=True)
            source.write_text('/* fn close_panel(zone: ZonePosition) {} */', encoding="utf-8")
            self.assertEqual(check_desktop_layout(root), [])

    def test_desktop_modules_do_not_import_other_features_by_path(self):
        violations = [
            path.relative_to(ROOT).as_posix()
            for path in (ROOT / "apps/desktop/ui/src").rglob("*.rs")
            if re.search(r"#\s*\[\s*path\s*=", path.read_text(encoding="utf-8"))
        ]
        self.assertEqual(violations, [], "通常の module 境界で Feature を接続してください")

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
