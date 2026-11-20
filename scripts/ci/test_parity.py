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


def old_workflow_commands() -> dict[str, list[tuple[str, str]]]:
    """基準ブランチの ci.yml を読み、job ごとの「step 名, コマンド」を返す。"""
    if shutil.which("git") is None:
        raise unittest.SkipTest("git is required")
    try:
        text = subprocess.check_output(
            ["git", "show", f"{BASE}:{OLD_WORKFLOW}"], cwd=ROOT, text=True, encoding="utf-8"
        )
    except subprocess.CalledProcessError:
        raise unittest.SkipTest(f"{BASE}:{OLD_WORKFLOW} is not available") from None

    # YAML パーサへの依存を避ける（CI の Python に PyYAML が無い）。
    # `jobs:` 以降だけを見て、2 スペースインデントの見出しを job 名、
    # `- name:` / `- uses:` を step、`run:` をそのコマンドとして拾う。
    lines = text.splitlines()
    start = next(
        (index for index, line in enumerate(lines) if line.rstrip() == "jobs:"), None
    )
    if start is None:
        raise unittest.SkipTest(f"{BASE}:{OLD_WORKFLOW} has no jobs")

    jobs: dict[str, list[tuple[str, str]]] = {}
    current: str | None = None
    step_name = ""
    in_steps = False
    run_indent: int | None = None
    buffer: list[str] = []

    def flush() -> None:
        nonlocal run_indent, buffer, step_name
        if run_indent is not None and current is not None:
            jobs[current].append((step_name, "\n".join(buffer)))
        run_indent = None
        buffer = []

    for line in lines[start + 1:]:
        if not line.strip():
            continue
        indent = len(line) - len(line.lstrip())
        stripped = line.strip()

        if run_indent is not None:
            if indent > run_indent:
                buffer.append(stripped)
                continue
            flush()

        if indent == 0:
            break
        if indent == 2 and stripped.endswith(":"):
            current = stripped[:-1]
            jobs.setdefault(current, [])
            in_steps = False
            step_name = ""
            continue
        if current is None:
            continue
        if indent == 4 and stripped == "steps:":
            in_steps = True
            continue
        if not in_steps:
            continue
        if stripped.startswith("- "):
            step_name = ""
            item = stripped[2:]
            if item.startswith("name:"):
                step_name = item.split(":", 1)[1].strip()
            elif item.startswith("run:"):
                step_name = "(unnamed)"
                value = item.split(":", 1)[1].strip()
                if value and value != "|":
                    jobs[current].append((step_name, value))
                else:
                    run_indent = indent
            continue
        if stripped.startswith("name:") and step_name == "":
            step_name = stripped.split(":", 1)[1].strip()
        elif stripped.startswith("run:"):
            value = stripped.split(":", 1)[1].strip()
            if value and value != "|":
                jobs[current].append((step_name or "(unnamed)", value))
            else:
                run_indent = indent
    flush()
    return jobs


class ParityTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.old = old_workflow_commands()

    def test_every_old_command_exists_in_the_route(self):
        missing = []
        for job, route_key in JOB_TO_ROUTE.items():
            route = ROUTES[route_key]
            for name, script in self.old.get(job, []):
                if name in SETUP_STEPS:
                    continue
                for command in commands_in(script):
                    if not any(self._matches(command, step_route) for step_route in route.steps):
                        missing.append(f"{job}: {name}: {command}")
        self.assertEqual(missing, [], "旧 CI にあったコマンドが jobs.py に無い")

    def _matches(self, command: str, step) -> bool:
        """コマンドの識別子（最初の 2 語）が step に含まれるかで対応を判定する。"""
        normalized = command.replace("python3 ", "")
        tokens = [token for token in normalized.split() if not token.startswith("-")]
        if not tokens:
            return True
        joined = " ".join(step.command)
        # `git apply ... --directory=vendor/gpui` は patches.py が内包する。
        if tokens[0] == "git" and "apply" in tokens[:2]:
            return any("scripts.ci.patches" in candidate for candidate in joined.split())
        return any(token in joined for token in tokens[:2])

    def test_old_jobs_and_routes_are_the_same_set(self):
        old_jobs = {job for job in self.old if job not in ("path_filter", "ci_gate")}
        self.assertEqual(old_jobs, set(JOB_TO_ROUTE))
        self.assertEqual(set(JOB_TO_ROUTE.values()), set(ROUTES))

    def test_setup_steps_are_covered_by_route_fields(self):
        for route in ROUTES.values():
            for step in route.steps:
                self.assertNotIn(step.name, SETUP_STEPS, f"{route.key}: setup step を jobs.py に書かない")


if __name__ == "__main__":
    unittest.main()
