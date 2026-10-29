"""scripts/ci/paths.py のユニットテスト。"""

import unittest
from scripts.ci.paths import jobs_for_paths, PATH_RULES, ALL_JOBS


class TestPathRouting(unittest.TestCase):
    """path routingロジックのテスト。"""

    def test_desktop_path_runs_desktop(self):
        self.assertIn("desktop", jobs_for_paths(["apps/desktop/app/src/main.rs"]))

    def test_host_path_runs_host(self):
        jobs = jobs_for_paths(["backend/host/src/main.rs"])
        self.assertIn("host", jobs)

    def test_rust_core_path_runs_rust_core(self):
        jobs = jobs_for_paths(["shared/models/workspace/src/lib.rs"])
        self.assertIn("rust_core", jobs)

    def test_mobile_path_runs_mobile(self):
        self.assertIn("mobile", jobs_for_paths(["apps/mobile/sharedUi/src/main.kt"]))

    def test_web_path_runs_web(self):
        self.assertIn("web", jobs_for_paths(["apps/web/package.json"]))

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

    def test_empty_paths_runs_nothing(self):
        self.assertEqual(jobs_for_paths([]), set())

    def test_infrastructure_runs_rust_core_and_host(self):
        """infrastructureはRust CoreとHostの両方に影響する。"""
        jobs = jobs_for_paths(["backend/infrastructure/local/src/lib.rs"])
        self.assertIn("rust_core", jobs)
        self.assertIn("host", jobs)

    def test_scripts_run_repository(self):
        self.assertIn("repository", jobs_for_paths(["scripts/check.py"]))

    def test_ci_workflow_runs_repository(self):
        self.assertIn("repository", jobs_for_paths([".github/workflows/ci.yml"]))

    def test_gpui_patch_runs_desktop(self):
        self.assertIn("desktop", jobs_for_paths(["patches/gpui-0.2.2-windows.patch"]))

    def test_multiple_paths_combine_jobs(self):
        """複数のpathが異なるjobを要求する場合、合体される。"""
        jobs = jobs_for_paths([
            "apps/desktop/app/src/main.rs",
            "backend/host/src/main.rs",
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


if __name__ == "__main__":
    unittest.main()
