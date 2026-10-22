"""開発者とCIが同じRust検証を実行するための入口。"""

import argparse
from pathlib import Path
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[1]
STAGES = ("architecture", "format", "check", "test", "lint", "build")


def commands(surface, stage):
    if stage == "architecture":
        if surface == "desktop":
            return [[sys.executable, "-m", "unittest", "discover", "-s", "scripts", "-p", "test_dev_commands.py"]]
        return [
            [sys.executable, "-m", "unittest", "discover", "-s", "scripts", "-p", "test_*.py"],
            [sys.executable, "scripts/check_architecture.py"],
        ]
    if stage == "format":
        return [["cargo", "fmt", "--all", "--", "--check"]] if surface == "backend" else []
    action = "clippy" if stage == "lint" else stage
    if surface == "backend":
        scope = ["--workspace", "--all-targets", "--exclude", "lapis", "--exclude", "lapis-desktop-ui"]
    else:
        scope = ["-p", "lapis", "--all-targets"]
        if stage != "build":
            scope += ["-p", "lapis-desktop-ui"]
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
    parser.add_argument("surface", choices=("backend", "desktop"))
    parser.add_argument("stage", nargs="?", choices=STAGES)
    args = parser.parse_args()
    try:
        run(args.surface, args.stage)
    except subprocess.CalledProcessError as error:
        return error.returncode
    return 0


if __name__ == "__main__":
    sys.exit(main())
