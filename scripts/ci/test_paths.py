"""scripts/ci/paths.py のユニットテスト。"""

import unittest
import sys
import subprocess
import tempfile
import re
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from scripts.ci.routes import ORDER, ROUTES
from scripts.ci.paths import ALL_JOBS, PATH_RULES, jobs_for_paths, worktree_paths
from scripts.ci.gate import errors
from scripts.ci.paths import changed_paths


class TestPathRouting(unittest.TestCase):
    """path routingロジックのテスト。"""

    def test_client_persistence_and_locales_select_rust_core(self):
        self.assertEqual(jobs_for_paths(["clients/state/src/lib.rs"]), {"desktop", "rust_core", "web"})
        self.assertEqual(jobs_for_paths(["apps/desktop/persistence/src/lib.rs"]), {"desktop", "rust_core"})
        self.assertEqual(jobs_for_paths(["locales/ja-JP/messages/common.json"]), {"repository", "rust_core", "desktop"})

    def test_route_names_match_the_workflow(self):
        """route の表示名は CI のジョブ名になるため、jobs.py を正とする。"""
        workflow = (Path(__file__).resolve().parents[2] / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        for key in ORDER:
            self.assertIn(key, ROUTES)
        for key in ORDER:
            self.assertIn(ROUTES[key].name, {route.name for route in ROUTES.values()})
        self.assertIn("name: Plan", workflow)
        self.assertIn("name: Gate", workflow)
        self.assertIn("python -m scripts.ci.run", workflow)

    def test_all_jobs_matches_route_keys(self):
        """path ルール側の job 名が jobs.py の route key と一致する。"""
        self.assertEqual(ALL_JOBS, set(ROUTES))

    def test_format_is_centralized_in_one_route(self):
        """整形チェックは 1 route に集約し、rustfmt を導入する。"""
        format_routes = [
            route
            for route in ROUTES.values()
            if any(any("fmt" in part for part in step.command) for step in route.steps)
        ]
        self.assertEqual([route.key for route in format_routes], ["format"])
        self.assertRegex(ROUTES["format"].rust_components, r"\brustfmt\b")

    def test_rust_core_installs_analyzer_for_platform_tests(self):
        rust_core = ROUTES["rust_core"]
        self.assertRegex(rust_core.rust_components, r"\brust-analyzer\b")
        self.assertRegex(rust_core.rust_components, r"\brust-src\b")

    def test_desktop_path_runs_desktop(self):
        self.assertIn("desktop", jobs_for_paths(["apps/desktop/app/src/main.rs"]))

    def test_extension_paths_run_both_backends(self):
        """ACP などの拡張は Host と Client の両方から参照するため両方の job を実行する。"""
        self.assertEqual(
            jobs_for_paths(["host/extensions/acp/src/session.rs"]),
            {"rust_core", "host", "desktop"},
        )
        self.assertEqual(
            jobs_for_paths(["extensions/acp/contract/src/lib.rs"]),
            {"repository", "rust_core", "host", "desktop", "web"},
        )

    def test_new_extension_paths_run_repository_and_all_rust_surfaces(self):
        """拡張の事前登録がなくても、コードと宣言・アセットを検証する。"""
        expected = {"repository", "rust_core", "host", "desktop", "web"}
        for path in (
            "extensions/new-plugin/manifest.json",
            "extensions/new-plugin/host/src/lib.rs",
            "extensions/new-plugin/assets/messages/ja.json",
            "extensions/terminal/manifest.json",
            "extensions/terminal/ui/src/lib.rs",
            "extensions/terminal/assets/messages/ja.json",
        ):
            self.assertEqual(jobs_for_paths([path]), expected, path)

    def test_extension_packaging_scripts_also_run_host_tests(self):
        self.assertEqual(jobs_for_paths(["scripts/extensions/package.py"]), {"repository", "rust_core", "host"})

    def test_terminal_extension_runs_only_the_surfaces_it_affects(self):
        """Terminal 拡張は契約と実装で影響する surface が分かれる。"""
        # 契約は Client からも参照するため wasm を含む Web も検証する。
        self.assertEqual(
            jobs_for_paths(["extensions/terminal/contract/src/lib.rs"]),
            {"rust_core", "host", "desktop", "web"},
        )
        # 独立ロジックは native と component の両経路で使う。
        self.assertEqual(
            jobs_for_paths(["extensions/terminal/logic/src/lib.rs"]),
            {"repository", "rust_core", "host", "desktop", "web"},
        )
        self.assertEqual(
            jobs_for_paths(["protocol/extension-api/native/src/lib.rs"]),
            {"rust_core", "host", "desktop"},
        )
        for path in (
            "extensions/terminal/adapter/src/lib.rs",
            "extensions/terminal/adapter/Cargo.toml",
        ):
            self.assertEqual(
                jobs_for_paths([path]), {"rust_core", "host", "desktop"}, path
            )

    def test_terminal_extension_non_contract_paths_stay_off_web(self):
        """契約以外の拡張ファイルは wasm ビルドの対象外として扱う。"""
        self.assertEqual(
            jobs_for_paths(["extensions/terminal/README.md"]), {"rust_core", "host", "desktop"}
        )

    def test_ui_path_runs_desktop_and_web(self):
        """clients/ui は desktop-ui として wasm でもビルドされるため Web ジョブも実行する。"""
        for path in ("clients/ui/src/shell/layout.rs", "clients/ui/assets/themes/dark.json"):
            self.assertEqual(jobs_for_paths([path]), {"desktop", "web"}, path)

    def test_state_path_runs_web_for_wasm_build(self):
        """clients/state は wasm ビルドの依存に入るため Web ジョブも実行する。"""
        self.assertEqual(jobs_for_paths(["clients/state/src/lib.rs"]), {"desktop", "rust_core", "web"})

    def test_web_terminal_extension_runs_windows_integration(self):
        for path in (
            "clients/state/src/web_host/extension_terminal.rs",
            "clients/state/src/terminal_extension/web_projection.rs",
            "apps/web/wasm/src/entry.rs",
            "apps/web/wasm/src/startup.rs",
            "clients/ui/wasm-smoke/src/lib.rs",
            "apps/web/wasm/tests/extension_terminal_smoke.py",
        ):
            self.assertIn("host", jobs_for_paths([path]), path)

    def test_wasm_build_dependency_paths_run_web(self):
        """desktop-ui の wasm 依存 crate の変更で Web ジョブが選ばれる。"""
        for path in (
            "clients/ui/src/lib.rs",
            "clients/state/src/lib.rs",
            "protocol/wire-client/src/lib.rs",
            "protocol/wire/client-api/src/lib.rs",
            "protocol/wire/dto/src/lib.rs",
            "protocol/types/document/src/lib.rs",
            "protocol/types/extension-ui/src/lib.rs",
        ):
            self.assertIn("web", jobs_for_paths([path]), path)

    def test_host_internal_path_runs_only_host(self):
        self.assertEqual(jobs_for_paths(["host/app/src/main.rs"]), {"host"})
        self.assertEqual(jobs_for_paths(["host/app/src/terminal.rs"]), {"host"})

    def test_host_readme_runs_only_repository(self):
        """Host の README は記述だけなので、コードの job を起こさない。"""
        self.assertEqual(jobs_for_paths(["host/README.md"]), {"repository"})

    def test_deleted_host_crate_paths_stay_in_the_rust_surfaces(self):
        """移動で消えた旧 path でも全 job を起こさない。"""
        for path in ("host/terminal/src/lib.rs", "host/adapters/terminal/src/lib.rs"):
            self.assertEqual(
                jobs_for_paths([path]), {"rust_core", "host", "desktop"}, path
            )

    def test_host_public_boundary_runs_desktop(self):
        for path in (
            "host/app/Cargo.toml",
            "host/app/src/lib.rs",
            "host/app/src/client.rs",
            "host/app/src/profile.rs",
        ):
            self.assertEqual(jobs_for_paths([path]), {"host", "desktop"})

    def test_protocol_and_client_transport_keep_cross_surface_ci(self):
        expected = {"rust_core", "host", "desktop", "web"}
        self.assertEqual(jobs_for_paths(["protocol/wire/client-api/src/lib.rs"]), expected)
        self.assertEqual(jobs_for_paths(["protocol/wire-client/src/lib.rs"]), expected)
        self.assertEqual(jobs_for_paths(["protocol/wire/dto/src/lib.rs"]), expected)

    def test_legacy_contract_paths_stay_in_the_same_surfaces(self):
        """移設前の path は削除差分として現れるため、全 job を起こさない。"""
        expected = {"rust_core", "host", "desktop", "web"}
        for path in (
            "contracts/protocol/src/lib.rs",
            "contracts/transport/wire/src/lib.rs",
            "contracts/domain/document/src/lib.rs",
            "clients/transport/src/lib.rs",
        ):
            self.assertEqual(jobs_for_paths([path]), expected, path)
        self.assertEqual(
            jobs_for_paths(["contracts/extension-api/native/src/lib.rs"]),
            {"rust_core", "host", "desktop"},
        )

    def test_protocol_readmes_run_only_repository(self):
        for path in ("protocol/README.md", "protocol/extension-api/README.md"):
            self.assertEqual(jobs_for_paths([path]), {"repository"}, path)

    def test_backend_transport_runs_rust_surfaces(self):
        expected = {"rust_core", "host", "desktop"}
        self.assertEqual(jobs_for_paths(["host/transport/remote/src/lib.rs"]), expected)

    def test_web_page_server_runs_web_and_rust_surfaces(self):
        expected = {"rust_core", "host", "desktop", "web"}
        self.assertEqual(jobs_for_paths(["host/transport/web/src/main.rs"]), expected)

    def test_rust_core_path_runs_rust_core(self):
        jobs = jobs_for_paths(["protocol/types/workspace/src/lib.rs"])
        self.assertIn("rust_core", jobs)

    def test_mobile_path_runs_mobile(self):
        self.assertIn("mobile", jobs_for_paths(["clients/mobile/sharedUi/src/main.kt"]))

    def test_web_path_runs_web(self):
        for path in ("apps/web/package.json", "apps/web/wasm/src/lib.rs"):
            self.assertIn("web", jobs_for_paths([path]), path)

    def test_removed_web_path_stays_in_web(self):
        """移設前の path は削除差分で現れるため、全 job を起こさない。"""
        self.assertEqual(jobs_for_paths(["clients/web/package.json"]), {"web"})

    def test_docs_site_paths_run_only_the_docs_site(self):
        """サイトは触ったときだけ、そのサイトの route を回す。"""
        for path in (
            "sites/docs/astro.config.mjs",
            "sites/docs/package.json",
            "sites/docs/src/content/docs/index.md",
        ):
            self.assertEqual(jobs_for_paths([path]), {"sites_docs"}, path)

    def test_docs_specs_do_not_build_the_site(self):
        self.assertEqual(jobs_for_paths(["docs/specs/architecture.md"]), {"repository"})

    def test_cargo_toml_runs_all_rust(self):
        jobs = jobs_for_paths(["Cargo.toml"])
        self.assertIn("repository", jobs)
        self.assertIn("rust_core", jobs)
        self.assertIn("host", jobs)
        self.assertIn("desktop", jobs)

    def test_unknown_path_runs_all(self):
        """未知のpathは安全側に倒して全jobを実行する。"""
        jobs = jobs_for_paths(["unknown-file.txt"])
        self.assertEqual(jobs, ALL_JOBS | {"repository"})
        self.assertEqual(jobs_for_paths(["Cargo.toml.extra"]), ALL_JOBS)

    def test_empty_paths_runs_nothing(self):
        self.assertEqual(jobs_for_paths([]), set())

    def test_infrastructure_runs_rust_core_and_host(self):
        """infrastructureはRust CoreとHostの両方に影響する。"""
        jobs = jobs_for_paths(["host/adapters/local/src/lib.rs"])
        self.assertIn("rust_core", jobs)
        self.assertIn("host", jobs)

    def test_scripts_run_repository(self):
        self.assertIn("repository", jobs_for_paths(["scripts/check.py"]))

    def test_release_tooling_runs_repository(self):
        self.assertEqual(jobs_for_paths(["scripts/release/cli.py"]), {"repository"})

    def test_release_registry_runs_desktop_because_the_launcher_embeds_it(self):
        self.assertEqual(jobs_for_paths(["release/trusted-keys.json"]), {"repository", "desktop"})

    def test_launcher_runs_desktop_and_rust_core(self):
        self.assertEqual(jobs_for_paths(["apps/launcher/src/main.rs"]), {"desktop", "rust_core"})

    def test_ci_workflow_runs_repository(self):
        self.assertIn("repository", jobs_for_paths([".github/workflows/ci.yml"]))

    def test_gpui_patch_runs_desktop(self):
        self.assertIn("desktop", jobs_for_paths(["patches/gpui-0.2.2-windows.patch"]))

    def test_multiple_paths_combine_jobs(self):
        """複数のpathが異なるjobを要求する場合、合体される。"""
        jobs = jobs_for_paths([
            "apps/desktop/app/src/main.rs",
            "host/app/src/main.rs",
        ])
        self.assertIn("desktop", jobs)
        self.assertIn("host", jobs)

    def test_path_rules_are_exhaustive(self):
        """PATH_RULES の pattern が重複していないことを確認。"""
        seen = set()
        for patterns, _ in PATH_RULES:
            for pattern in patterns:
                self.assertNotIn(pattern, seen, f"重複するpattern: {pattern}")
                seen.add(pattern)

    def test_windows_dev_commands_are_selected(self):
        for path in ("scripts/stop_dev_host.ps1", "scripts/test_dev_commands.py", "scripts/test_dev_session.py", "scripts/dev-session/src/main.rs", ".run/Lapis.run.xml", ".cargo/config.toml"):
            self.assertIn("dev_commands", jobs_for_paths([path]))
        self.assertNotIn("dev_commands", jobs_for_paths(["scripts/other.py"]))

    def test_gate_checks_mapped_jobs_and_expected_skips(self):
        for route, path in (("desktop", "apps/desktop/app/src/main.rs"), ("host", "host/app/src/main.rs")):
            selected = sorted(jobs_for_paths([path]))
            needs = {"plan": {"result": "success"}, "ci": {"result": "success"}}
            self.assertEqual(errors(selected, needs, len(selected)), [])
            if not selected:
                self.assertTrue(errors(selected, needs, 1))
                continue
            for result in ("failure", "cancelled", "timed_out", "skipped"):
                needs["ci"]["result"] = result
                self.assertTrue(errors(selected, needs, len(selected)), result)
            needs["ci"]["result"] = "success"
            needs["plan"]["result"] = "failure"
            self.assertTrue(errors(selected, needs, len(selected)))

    def test_diff_uses_given_parent_for_stacked_pr_and_push(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def git(*args):
                return subprocess.check_output(["git", "-c", "user.name=Test", "-c", "user.email=test@example.invalid", "-c", "core.hooksPath=", *args], cwd=root).decode().strip()
            git("init")
            (root / "parent.txt").write_text("a")
            git("add", ".")
            git("commit", "-m", "base")
            base = git("rev-parse", "HEAD")
            (root / "parent.txt").write_text("b")
            git("commit", "-am", "parent")
            parent = git("rev-parse", "HEAD")
            (root / "child.txt").write_text("c")
            git("add", ".")
            git("commit", "-m", "child")
            child = git("rev-parse", "HEAD")
            with patch("scripts.ci.paths.ROOT", root):
                self.assertEqual(changed_paths(parent), ["child.txt"])
                self.assertEqual(set(changed_paths(base)), {"parent.txt", "child.txt"})
                git("mv", "child.txt", "renamed.txt")
                git("commit", "-m", "rename")
                self.assertEqual(set(changed_paths(child)), {"child.txt", "renamed.txt"})

    def test_diff_ignores_commits_that_only_the_base_has(self):
        """PR の差分は merge-base から見る（base 側の変更を削除と誤認しない）。"""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)

            def git(*args):
                return subprocess.check_output(
                    ["git", "-c", "user.name=Test", "-c", "user.email=test@example.invalid",
                     "-c", "core.hooksPath=", *args],
                    cwd=root,
                ).decode().strip()

            git("init")
            (root / "shared.txt").write_text("a")
            git("add", ".")
            git("commit", "-m", "base")
            base = git("rev-parse", "HEAD")

            # base 側にだけ進んだコミット。
            (root / "base-only.txt").write_text("x")
            git("add", ".")
            git("commit", "-m", "base only")
            moved_base = git("rev-parse", "HEAD")

            # base から分岐した feature。
            git("checkout", "-b", "feature", base)
            (root / "feature-only.txt").write_text("y")
            git("add", ".")
            git("commit", "-m", "feature")

            with patch("scripts.ci.paths.ROOT", root):
                self.assertEqual(
                    changed_paths(moved_base, include_worktree=False), ["feature-only.txt"]
                )

    def test_changed_paths_includes_uncommitted_and_untracked(self):
        """未コミットの変更と新規ファイルを PR の差分に足す。"""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)

            def git(*args):
                return subprocess.check_output(
                    ["git", "-c", "user.name=Test", "-c", "user.email=test@example.invalid",
                     "-c", "core.hooksPath=", *args],
                    cwd=root,
                ).decode().strip()

            git("init")
            (root / "tracked.txt").write_text("a")
            git("add", ".")
            git("commit", "-m", "base")

            (root / "tracked.txt").write_text("b")
            (root / "untracked.txt").write_text("new")

            with patch("scripts.ci.paths.ROOT", root):
                self.assertEqual(worktree_paths(), ["tracked.txt", "untracked.txt"])
                self.assertEqual(
                    changed_paths("HEAD", include_worktree=False), []
                )
                self.assertEqual(
                    changed_paths("HEAD"), ["tracked.txt", "untracked.txt"]
                )


if __name__ == "__main__":
    unittest.main()
