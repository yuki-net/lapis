"""scripts/ci/paths.py のユニットテスト。"""

import unittest
import sys
import subprocess
import tempfile
import re
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from scripts.ci.paths import jobs_for_paths, PATH_RULES, ALL_JOBS
from scripts.ci.gate import errors
from scripts.ci.paths import JOB_IDS
from scripts.ci.paths import changed_paths


class TestPathRouting(unittest.TestCase):
    """path routingロジックのテスト。"""

    def test_client_persistence_and_locales_select_rust_core(self):
        self.assertEqual(jobs_for_paths(["clients/state/src/lib.rs"]), {"desktop", "rust_core", "web"})
        self.assertEqual(jobs_for_paths(["clients/desktop/persistence/src/lib.rs"]), {"desktop", "rust_core"})
        self.assertEqual(jobs_for_paths(["locales/ja-JP/messages/common.json"]), {"repository", "rust_core", "desktop"})

    def test_ci_workflow_uses_final_check_names_and_no_release_validation(self):
        workflow = (Path(__file__).resolve().parents[2] / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        for name in ("Changes", "Dev Commands / Windows", "Mobile / Android", "Gate"):
            self.assertIn(f"name: {name}", workflow)
        self.assertNotIn("Build Release Desktop", workflow)
        self.assertNotIn("Test Release Inspector boundary", workflow)

    def test_format_jobs_install_rustfmt(self):
        workflow = (Path(__file__).resolve().parents[2] / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        jobs = re.split(r"(?m)^  (?=[a-z_]+:\s*$)", workflow.split("\njobs:\n", 1)[1])
        format_jobs = [job for job in jobs if re.search(r"scripts/check\.py\s+\w+\s+format", job)]
        self.assertEqual(len(format_jobs), 3)
        for job in format_jobs:
            self.assertRegex(job, r"(?m)^\s+components:\s+[^\n]*\brustfmt\b")

    def test_rust_core_installs_analyzer_for_platform_tests(self):
        workflow = (Path(__file__).resolve().parents[2] / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        rust_core = workflow.split("\n  rust_core:\n", 1)[1].split("\n  host_windows:\n", 1)[0]
        self.assertRegex(rust_core, r"(?m)^\s+components:\s+[^\n]*\brust-analyzer\b")
        self.assertRegex(rust_core, r"(?m)^\s+components:\s+[^\n]*\brust-src\b")

    def test_desktop_path_runs_desktop(self):
        self.assertIn("desktop", jobs_for_paths(["clients/desktop/app/src/main.rs"]))

    def test_ui_path_runs_desktop_and_web(self):
        """clients/ui は desktop-ui として wasm でもビルドされるため Web ジョブも実行する。"""
        for path in ("clients/ui/src/shell/layout.rs", "clients/ui/assets/themes/dark.json"):
            self.assertEqual(jobs_for_paths([path]), {"desktop", "web"}, path)

    def test_state_path_runs_web_for_wasm_build(self):
        """clients/state は wasm ビルドの依存に入るため Web ジョブも実行する。"""
        self.assertEqual(jobs_for_paths(["clients/state/src/lib.rs"]), {"desktop", "rust_core", "web"})

    def test_wasm_build_dependency_paths_run_web(self):
        """desktop-ui の wasm 依存 crate の変更で Web ジョブが選ばれる。"""
        for path in (
            "clients/ui/src/lib.rs",
            "clients/state/src/lib.rs",
            "clients/transport/src/lib.rs",
            "contracts/protocol/src/lib.rs",
            "contracts/transport/wire/src/lib.rs",
            "contracts/domain/document/src/lib.rs",
        ):
            self.assertIn("web", jobs_for_paths([path]), path)

    def test_host_internal_path_runs_only_host(self):
        self.assertEqual(jobs_for_paths(["host/app/src/main.rs"]), {"host"})
        self.assertEqual(jobs_for_paths(["host/app/src/terminal.rs"]), {"host"})

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
        self.assertEqual(jobs_for_paths(["contracts/protocol/src/lib.rs"]), expected)
        self.assertEqual(jobs_for_paths(["clients/transport/src/lib.rs"]), expected)
        self.assertEqual(jobs_for_paths(["contracts/transport/wire/src/lib.rs"]), expected)

    def test_backend_transport_runs_rust_surfaces(self):
        expected = {"rust_core", "host", "desktop"}
        self.assertEqual(jobs_for_paths(["host/transport/remote/src/lib.rs"]), expected)

    def test_web_page_server_runs_web_and_rust_surfaces(self):
        expected = {"rust_core", "host", "desktop", "web"}
        self.assertEqual(jobs_for_paths(["host/transport/web/src/main.rs"]), expected)

    def test_rust_core_path_runs_rust_core(self):
        jobs = jobs_for_paths(["contracts/domain/workspace/src/lib.rs"])
        self.assertIn("rust_core", jobs)

    def test_mobile_path_runs_mobile(self):
        self.assertIn("mobile", jobs_for_paths(["clients/mobile/sharedUi/src/main.kt"]))

    def test_web_path_runs_web(self):
        self.assertIn("web", jobs_for_paths(["clients/web/package.json"]))

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
        self.assertEqual(jobs_for_paths(["clients/desktop/launcher/src/main.rs"]), {"desktop", "rust_core"})

    def test_ci_workflow_runs_repository(self):
        self.assertIn("repository", jobs_for_paths([".github/workflows/ci.yml"]))

    def test_gpui_patch_runs_desktop(self):
        self.assertIn("desktop", jobs_for_paths(["patches/gpui-0.2.2-windows.patch"]))

    def test_multiple_paths_combine_jobs(self):
        """複数のpathが異なるjobを要求する場合、合体される。"""
        jobs = jobs_for_paths([
            "clients/desktop/app/src/main.rs",
            "host/app/src/main.rs",
        ])
        self.assertIn("desktop", jobs)
        self.assertIn("host", jobs)

    def test_path_rules_are_exhaustive(self):
        """PATH_RULESのpatternが重複していないことを確認。"""
        seen = set()
        for patterns, _ in PATH_RULES:
            for pattern in patterns:
                self.assertNotIn(pattern, seen, f"重複するpattern: {pattern}")
                seen.add(pattern)

    def test_windows_dev_commands_are_selected(self):
        for path in ("scripts/stop_dev_host.ps1", "scripts/test_dev_commands.py", "scripts/test_dev_session.py", "scripts/dev-session/src/main.rs", ".run/Lapis.run.xml", ".cargo/config.toml"):
            self.assertIn("dev_windows", jobs_for_paths([path]))
        self.assertNotIn("dev_windows", jobs_for_paths(["scripts/other.py"]))

    def test_gate_checks_mapped_jobs_and_expected_skips(self):
        for route, path in (("desktop", "clients/desktop/app/src/main.rs"), ("host", "host/app/src/main.rs")):
            selected = list(jobs_for_paths([path]))
            needs = {"path_filter": {"result": "success"}}
            needs.update({job: {"result": "success" if name in selected else "skipped"} for name, job in JOB_IDS.items()})
            self.assertEqual(errors(selected, needs), [])
            for result in ("failure", "cancelled", "timed_out", "skipped"):
                needs[JOB_IDS[route]]["result"] = result
                self.assertTrue(errors(selected, needs), result)
            needs[JOB_IDS[route]]["result"] = "success"
            needs["path_filter"]["result"] = "failure"
            self.assertTrue(errors(selected, needs))

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


if __name__ == "__main__":
    unittest.main()
