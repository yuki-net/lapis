import copy
import re
import tempfile
from pathlib import Path
import unittest

from check_architecture import check, check_desktop_layout, check_duplicate_type_names

ROOT = Path(__file__).resolve().parents[1]


def package(name, path, dependencies=()):
    return {"name": name, "id": name, "manifest_path": str(ROOT / path / "Cargo.toml"),
            "dependencies": [{"name": dependency} for dependency in dependencies]}


class ArchitectureTests(unittest.TestCase):
    def test_desktop_layout_contract_rejects_old_identifiers_and_feature_internals(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "clients/ui/src/features/example.rs"
            source.parent.mkdir(parents=True)
            source.write_text('// TabGroupId was removed\nconst TEXT: &str = "DropZone";\n', encoding="utf-8")
            self.assertEqual(check_desktop_layout(root), [])
            source.write_text('fn use_old(_: TabGroupId) {}\n', encoding="utf-8")
            self.assertTrue(any("TabGroupId" in error for error in check_desktop_layout(root)))
            source.write_text('use crate::shell::panel_layout::PanelLayout;\n', encoding="utf-8")
            self.assertTrue(any("Feature" in error for error in check_desktop_layout(root)))

    def test_feature_viewport_and_user_agent_access_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "clients/ui/src/features/example.rs"
            source.parent.mkdir(parents=True)
            source.write_text('// viewport_size() は共通 wrapper を使う\n', encoding="utf-8")
            self.assertEqual(check_desktop_layout(root), [])
            source.write_text('fn f(window: &Window) { window.viewport_size(); }\n', encoding="utf-8")
            self.assertTrue(
                any("viewport" in error for error in check_desktop_layout(root))
            )
            source.write_text('fn f() -> String { navigator.userAgent.into() }\n', encoding="utf-8")
            self.assertTrue(
                any("viewport" in error for error in check_desktop_layout(root))
            )

    def test_dialog_access_from_feature_and_root_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            feature = root / "clients/ui/src/features/example.rs"
            feature.parent.mkdir(parents=True)
            feature.write_text(
                '// file_dialog() は client wrapper を使う\n', encoding="utf-8"
            )
            self.assertEqual(check_desktop_layout(root), [])

            feature.write_text(
                'fn f(client: &Client) { client.editor.file_dialog(); }\n',
                encoding="utf-8",
            )
            self.assertTrue(
                any("Dialog" in error for error in check_desktop_layout(root))
            )

            feature.unlink()
            root_file = root / "clients/ui/src/root.rs"
            root_file.write_text(
                'fn f(client: &Client) { client.editor.file_dialog().choose_workspace_path(); }\n',
                encoding="utf-8",
            )
            self.assertTrue(
                any("Dialog" in error for error in check_desktop_layout(root))
            )

    def test_feature_manual_scroll_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            feature = root / "clients/ui/src/features/example.rs"
            feature.parent.mkdir(parents=True)
            feature.write_text(
                '// overflow_y_scroll() は scroll_viewport を使う\n', encoding="utf-8"
            )
            self.assertEqual(check_desktop_layout(root), [])

            feature.write_text('fn f() { div().overflow_y_scroll(); }\n', encoding="utf-8")
            self.assertTrue(
                any("scroll_viewport" in error for error in check_desktop_layout(root))
            )

            feature.write_text('fn f() { div().overflow_scroll(); }\n', encoding="utf-8")
            self.assertTrue(
                any("scroll_viewport" in error for error in check_desktop_layout(root))
            )

    def test_duplicate_type_names_across_crates_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            first = root / "contracts/domain/example/src/lib.rs"
            second = root / "host/runtime/src/lib.rs"
            for path, name in ((first, "example"), (second, "runtime")):
                path.parent.mkdir(parents=True, exist_ok=True)
                (path.parents[1] / "Cargo.toml").write_text(
                    f"[package]\nname = \"{name}\"\n", encoding="utf-8"
                )

            first.write_text("pub struct Shared;\n", encoding="utf-8")
            self.assertEqual(check_duplicate_type_names(root), [])

            second.write_text("pub struct Shared;\n", encoding="utf-8")
            self.assertTrue(
                any("Shared" in error for error in check_duplicate_type_names(root))
            )

    def test_duplicate_type_names_use_package_names_not_directories(self):
        """`extensions/*/contract` のようにディレクトリ名が同じ crate を区別する。"""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            first = root / "extensions/alpha/contract/src/lib.rs"
            second = root / "extensions/beta/contract/src/lib.rs"
            for path, name in ((first, "alpha-contract"), (second, "beta-contract")):
                path.parent.mkdir(parents=True, exist_ok=True)
                (path.parents[1] / "Cargo.toml").write_text(
                    f"[package]\nname = \"{name}\"\n", encoding="utf-8"
                )
            first.write_text("pub struct Shared;\n", encoding="utf-8")
            second.write_text("pub struct Shared;\n", encoding="utf-8")
            self.assertTrue(
                any("Shared" in error for error in check_duplicate_type_names(root))
            )

    def test_zone_persistence_rejects_legacy_names_and_helpers(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "clients/ui/src/shell/layout.rs"
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
            source = root / "clients/ui/src/shell/example.rs"
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
            package("model", "contracts/domain/example"),
            package("runtime", "host/runtime", ["model"]),
            package("ui", "clients/ui", ["model"]),
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
        for path in ("host/runtime", "host/app"):
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
            package("cli", "clients/cli", ["infra"]),
            package("infra", "host/adapters/local"),
        ])
        self.assertIn("cli -> infra", "\n".join(check(self.metadata, {})))

    def test_regular_client_can_depend_on_protocol(self):
        self.metadata["workspace_members"].extend(["cli", "protocol"])
        self.metadata["packages"].extend([
            package("cli", "clients/cli", ["protocol"]),
            package("protocol", "contracts/protocol"),
        ])
        self.assertEqual(check(self.metadata, {}), [])

    def test_desktop_entry_can_compose_backend(self):
        self.metadata["workspace_members"].extend(["desktop", "infra"])
        self.metadata["packages"].extend([
            package("desktop", "clients/desktop/app", ["ui", "runtime", "infra"]),
            package("infra", "host/adapters/local"),
        ])
        self.assertEqual(check(self.metadata, {}), [])

    def test_extension_role_directories_keep_their_layer(self):
        """自己完結した拡張でも `host` は runtime、`adapter` は infrastructure として検査する。"""
        self.metadata["workspace_members"].extend(
            ["terminal-contract", "terminal-state", "local-terminal"]
        )
        self.metadata["packages"].extend([
            package("terminal-contract", "extensions/terminal/contract"),
            package("terminal-state", "extensions/terminal/host", ["terminal-contract"]),
            package(
                "local-terminal",
                "extensions/terminal/adapter",
                ["terminal-contract"],
            ),
        ])
        self.assertEqual(check(self.metadata, {}), [])

    def test_extension_contract_cannot_depend_on_host_state(self):
        self.metadata["workspace_members"].extend(["terminal-contract", "terminal-state"])
        self.metadata["packages"].extend([
            package("terminal-contract", "extensions/terminal/contract", ["terminal-state"]),
            package("terminal-state", "extensions/terminal/host"),
        ])
        self.assertIn(
            "terminal-contract -> terminal-state",
            "\n".join(check(self.metadata, {})),
        )

    def test_desktop_ui_cannot_depend_on_infrastructure(self):
        self.metadata["workspace_members"].append("infra")
        self.metadata["packages"].append(package("infra", "host/adapters/local"))
        self.metadata["packages"][2]["dependencies"].append({"name": "infra"})
        self.assertIn("ui -> infra", "\n".join(check(self.metadata, {})))

    def test_only_desktop_composition_root_can_depend_on_host(self):
        self.metadata["workspace_members"].extend(["desktop", "cli", "host"])
        self.metadata["packages"].extend([
            package("desktop", "clients/desktop/app", ["host"]),
            package("cli", "clients/cli"),
            package("host", "host/app", ["runtime"]),
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
            package("desktop", "clients/desktop/app", ["ui", "adapter", "store"]),
            package("adapter", "clients/state", ["model"]),
            package("store", "clients/desktop/persistence", ["adapter", "model"]),
            package("infra", "host/adapters/local", ["runtime"]),
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
        for path in ("clients/state", "clients/desktop/persistence"):
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
            source = root / "clients/ui/src/shell/example.rs"
            source.parent.mkdir(parents=True)
            source.write_text('// fn close_panel(zone: ZonePosition) {}\n', encoding="utf-8")
            self.assertEqual(check_desktop_layout(root), [])

    def test_real_old_zone_api_is_rejected(self):
        """実コードの旧Zone API署名はエラーにする。"""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "clients/ui/src/shell/example.rs"
            source.parent.mkdir(parents=True)
            source.write_text('fn close_panel(zone: ZonePosition) {}', encoding="utf-8")
            self.assertTrue(any("close_panel" in error for error in check_desktop_layout(root)))

    def test_comment_panelhost_position_is_allowed(self):
        """コメント内のPanelHost.positionはエラーにしない。"""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "clients/ui/src/shell/example.rs"
            source.parent.mkdir(parents=True)
            source.write_text('// struct PanelHost { position: ZonePosition }', encoding="utf-8")
            self.assertEqual(check_desktop_layout(root), [])

    def test_real_panelhost_position_is_rejected(self):
        """実コードのPanelHost.positionはエラーにする。"""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "clients/ui/src/shell/example.rs"
            source.parent.mkdir(parents=True)
            source.write_text('struct PanelHost { position: ZonePosition }', encoding="utf-8")
            self.assertTrue(any("二重保持" in error for error in check_desktop_layout(root)))

    def test_comment_old_zone_save_name_is_allowed(self):
        """コメント内の旧Zone保存名はエラーにしない。"""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "clients/ui/src/shell/layout.rs"
            source.parent.mkdir(parents=True)
            source.write_text('// legacy value: "left-panel"', encoding="utf-8")
            self.assertEqual(check_desktop_layout(root), [])

    def test_real_old_zone_save_name_is_rejected(self):
        """実文字列の旧Zone保存名はエラーにする。"""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "clients/ui/src/shell/layout.rs"
            source.parent.mkdir(parents=True)
            source.write_text('const VALUE: &str = "left-panel";', encoding="utf-8")
            self.assertTrue(any("left-panel" in error for error in check_desktop_layout(root)))

    def test_block_comment_old_zone_api_is_allowed(self):
        """ブロックコメント内の旧Zone API署名もエラーにしない。"""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "clients/ui/src/shell/example.rs"
            source.parent.mkdir(parents=True)
            source.write_text('/* fn close_panel(zone: ZonePosition) {} */', encoding="utf-8")
            self.assertEqual(check_desktop_layout(root), [])

    def test_desktop_modules_do_not_import_other_features_by_path(self):
        violations = [
            path.relative_to(ROOT).as_posix()
            for path in (ROOT / "clients/ui/src").rglob("*.rs")
            if re.search(r"#\s*\[\s*path\s*=", path.read_text(encoding="utf-8"))
        ]
        self.assertEqual(violations, [], "通常の module 境界で Feature を接続してください")

    def test_backend_host_cannot_depend_on_gpui_or_rfd(self):
        for dependency in ("gpui", "rfd"):
            with self.subTest(dependency=dependency):
                metadata = copy.deepcopy(self.metadata)
                metadata["workspace_members"].append("host")
                metadata["packages"].append(
                    package("host", "host/app", [dependency])
                )
                self.assertIn(
                    f"host -> {dependency}: Client 固有依存",
                    check(metadata, {}),
                )


if __name__ == "__main__":
    unittest.main()
