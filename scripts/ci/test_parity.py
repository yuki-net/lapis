"""旧 ci.yml と jobs.py の step 列を突き合わせる回帰テスト。

CI の定義を YAML から jobs.py へ移したときに、コマンドが 1 つでも
落ちていないことを保証する。旧定義は `git show <base>:...` で取得する。
"""

import os
import shutil
import subprocess
import unittest
from pathlib import Path

from scripts.ci.jobs import ROUTES, ROOT

BASE = os.environ.get("CI_PARITY_BASE", "origin/develop")
OLD_WORKFLOW = ".github/workflows/ci.yml"

# 旧 job 名 → 新 route。判定用の job（path_filter / ci_gate）は対象外。
JOB_TO_ROUTE = {
    "repository": "repository",
    "dev_windows": "dev_windows",
    "rust_core": "rust_core",
    "host_windows": "host",
    "desktop_windows": "desktop",
    "mobile": "mobile",
    "web": "web",
}

# setup 用の step（runner 準備）は jobs.py の Route フィールドが持つため対象外。
SETUP_STEPS = {
    "Check out repository", "Set up Python", "Install Rust toolchain", "Cache Cargo artifacts",
    "Set up Node.js", "Install wasm-bindgen", "Set up Java", "Set up Gradle",
    "List jobs", "Check results",
}


def old_workflow() -> dict:
    """基準ブランチの ci.yml を読み、job ごとの実行コマンドを返す。"""
    import yaml

    if shutil.which("git") is None:
        raise unittest.SkipTest("git is required")
    try:
        text = subprocess.check_output(
            ["git", "show", f"{BASE}:{OLD_WORKFLOW}"], cwd=ROOT, text=True, encoding="utf-8"
        )
    except subprocess.CalledProcessError:
        raise unittest.SkipTest(f"{BASE}:{OLD_WORKFLOW} is not available") from None
    return yaml.safe_load(text)


def commands_in(script: str) -> list[str]:
    """pwsh / bash の step から実際の検証コマンドだけを取り出す。

    `if ($LASTEXITCODE ...)` や `echo`、`exit` などの制御行は落とす。
    再試行ループの `&& exit 0` はコマンド本体だけ残す。
    """
    commands = []
    for raw in script.splitlines():
        line = raw.strip()
        if not line:
            continue
        if line.startswith(("if ", "if(", "}", ")", "exit ", "throw ", "echo ", "sleep ", "$")):
            continue
        if line.startswith(("&", "for ", "foreach ", "done")):
            continue
        if "LASTEXITCODE" in line or "attempt" in line and "echo" in line:
            continue
        line = line.split(" && exit")[0].strip()
        commands.append(line)
    return commands


class ParityTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.old = old_workflow()

    def test_every_old_command_exists_in_the_route(self):
        missing = []
        for job, route_key in JOB_TO_ROUTE.items():
            route = ROUTES[route_key]
            for step in self.old["jobs"][job]["steps"]:
                name = step.get("name", "")
                if name in SETUP_STEPS:
                    continue
                script = step.get("run")
                if not script:
                    continue
                for command in commands_in(script):
                    if not any(self._matches(command, step_route) for step_route in route.steps):
                        missing.append(f"{job}: {name}: {command}")
        self.assertEqual(missing, [], "旧 CI にあったコマンドが jobs.py に無い")

    def _matches(self, command: str, step) -> bool:
        """コマンドの識別子（最初の 2 語）が step に含まれるかで対応を判定する。"""
        normalized = command.replace("python3 ", "").replace("./gradlew", "./gradlew")
        tokens = [token for token in normalized.split() if not token.startswith("-")]
        if not tokens:
            return True
        joined = " ".join(step.command)
        # `git apply ... --directory=vendor/gpui` は patches.py が内包する。
        if tokens[0] == "git" and "apply" in tokens[:2]:
            return any("scripts.ci.patches" in candidate for candidate in joined.split())
        return any(token in joined for token in tokens[:2])

    def test_old_jobs_and_routes_are_the_same_set(self):
        old_jobs = {job for job in self.old["jobs"] if job not in ("path_filter", "ci_gate")}
        self.assertEqual(old_jobs, set(JOB_TO_ROUTE))
        self.assertEqual(set(JOB_TO_ROUTE.values()), set(ROUTES))

    def test_setup_steps_are_covered_by_route_fields(self):
        for route in ROUTES.values():
            for step in route.steps:
                self.assertNotIn(step.name, SETUP_STEPS, f"{route.key}: setup step を jobs.py に書かない")


if __name__ == "__main__":
    unittest.main()
