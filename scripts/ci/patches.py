"""GPUI patch の適用状態を検証する。

vendor/gpui が patches/ の内容と一致していることを、逆適用の成否で確かめる。
CI と同じ 3 パッチを対象にする。
"""

import subprocess
import sys
from pathlib import Path

from scripts.ci.console import configure

ROOT = Path(__file__).resolve().parents[2]
PATCHES = ("external-inspector", "windows", "drag")


def main() -> int:
    configure()
    for patch in PATCHES:
        command = [
            "git", "apply",
            "--unidiff-zero",
            "--ignore-space-change",
            "--check",
            "--reverse",
            "--directory=vendor/gpui",
            f"patches/gpui-0.2.2-{patch}.patch",
        ]
        result = subprocess.run(command, cwd=ROOT)
        if result.returncode != 0:
            print(f"vendor/gpui does not match patches/gpui-0.2.2-{patch}.patch", file=sys.stderr)
            return result.returncode
    return 0


if __name__ == "__main__":
    sys.exit(main())
