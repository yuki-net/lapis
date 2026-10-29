"""開発者とCIが同じ検証を実行するための入口。

surfaceは repository / rust_core / host / desktop のいずれか。
backend は rust_core + host のエイリアスとして残す。
"""

import argparse
from pathlib import Path
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[1]
STAGES = ("architecture", "format", "check", "test", "lint", "build")

# surface → cargo scopeのマップ
SURFACE_SCOPES = {
    "repository": [],  # repositoryはcargoコマンドを使わない
    "rust_core": ["--workspace", "--all-targets", "--exclude", "lapis", "--exclude", "lapis-desktop-ui", "--exclude", "lapis-host"],
    "host": ["-p", "lapis-host", "--all-targets"],
    "desktop": ["-p", "lapis", "--all-targets", "-p", "lapis-desktop-ui"],
    "backend": ["--workspace", "--all-targets", "--exclude", "lapis", "--exclude", "lapis-desktop-ui"],
}


def commands(surface, stage):
    if stage == "architecture":
        cmds = []
        if surface == "repository":
            cmds.append([sys.executable, "-m", "unittest", "discover", "-s", "scripts/ci", "-p", "test_*.py"])
        if surface in ("repository", "desktop"):
            cmds.append([sys.executable, "-m", "unittest", "discover", "-s", "scripts", "-p", "test_dev_commands.py"])
        if surface in ("repository", "backend", "rust_core", "host"):
            cmds.append([sys.executable, "-m", "unittest", "discover", "-s", "scripts", "-p", "test_*.py"])
        cmds.append([sys.executable, "scripts/check_architecture.py"])
        return cmds if cmds else [[sys.executable, "-c", "pass"]]
    if stage == "format":
        return [["cargo", "fmt", "--all", "--", "--check"]]
    
    # repositoryはcargoコマンドを持たない
    if surface == "repository":
        return [[sys.executable, "-c", "pass"]]
    
    action = "clippy" if stage == "lint" else stage
    scope = SURFACE_SCOPES.get(surface, SURFACE_SCOPES["backend"])
    command = ["cargo", action, "--locked", *scope]
    if stage == "lint":
        command += ["--", "-D", "warnings"]
    return [command]


def run(surface, stage=None):
    for current in (stage,) if stage else STAGES:
        for command in commands(surface, current):
            print("+ " + " ".join(command), flush=True)
            subprocess.run(command, cwd=ROOT, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("surface", choices=("repository", "rust_core", "host", "desktop", "backend"))
    parser.add_argument("stage", nargs="?", choices=STAGES)
    args = parser.parse_args()
    try:
        run(args.surface, args.stage)
    except subprocess.CalledProcessError as error:
        return error.returncode
    return 0


if __name__ == "__main__":
    sys.exit(main())
