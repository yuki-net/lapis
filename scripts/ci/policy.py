"""保護パス（lint 基盤・CI 定義・自動化）の変更を検出して通知する。

保護パスは「どの変更 path でどの検証を走らせるか」ではなく、PR 全体の性質な
ので route にはしない。GitHub では pull_request のときだけ job として走らせ、
変更があれば notice と job summary で知らせる。**マージのゲートは CODEOWNERS
（@yuki-net/reviews）**で、このチェックは失敗させない（CI の赤を本物の失敗だけ
にするため）。

判定を変えるにはこのファイルを変える必要があり、それ自体が保護パスなので
必ず人間が差分を確認してマージする。
"""

import argparse
import fnmatch
import os
import sys

from scripts.ci import report
from scripts.ci.console import configure
from scripts.ci.paths import pr_paths

# 変更されると「lint / CI / 自動化の仕組み」自体が変わる path。fnmatch なので
# `lints/*` は配下のすべてに一致する。
PROTECTED = (
    "lints/*",
    "AGENTS.md",
    ".github/*",
    "automation/*",
    "reviewer/*",
    "docs/specs/automation.md",
    ".opencode/skills/build-issue/*",
    ".opencode/skills/review-pr/*",
    "scripts/check*.py",
    "scripts/architecture-exceptions.json",
    "scripts/ci/*",
    "clippy.toml",
    "rust-toolchain.toml",
    "rustfmt.toml",
)

GUIDE = "保護パスの変更は CODEOWNERS の承認（@yuki-net/reviews）が必要（lints/catalog/protected-paths.md）"


def pattern_for(path: str) -> str | None:
    """path が保護パスなら、一致した規則を返す。"""
    for pattern in PROTECTED:
        if fnmatch.fnmatchcase(path, pattern):
            return pattern
    return None


def protected_hits(paths: list[str]) -> list[tuple[str, str]]:
    """保護パスに触れた (path, 規則) を返す。"""
    hits = []
    for path in paths:
        pattern = pattern_for(path)
        if pattern is not None:
            hits.append((path, pattern))
    return hits


def resolve_base(argument: str | None) -> str:
    """diff の基準 ref を決める。

    優先順位: `--base` → `LAPIS_POLICY_BASE` → PR の base ブランチ →
    `origin/develop`。CI では actions/checkout が全履歴を取るので、
    `origin/<base>` がそのまま使える。
    """
    if argument:
        return argument
    explicit = os.environ.get("LAPIS_POLICY_BASE")
    if explicit:
        return explicit
    base_ref = os.environ.get("GITHUB_BASE_REF")
    if base_ref:
        return f"origin/{base_ref}"
    return "origin/develop"


def main() -> int:
    configure()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", default=None, help="diff の基準 ref。既定は環境変数から")
    args = parser.parse_args()

    base = resolve_base(args.base)
    paths = pr_paths(base)
    if paths is None:
        print(f"::error::変更 path を取得できない（base: {base}）", file=sys.stderr)
        return 2

    hits = protected_hits(paths)
    if not hits:
        print(f"protected paths: 変更なし（{len(paths)} path、base: {base}）")
        return 0

    print(f"protected paths: {len(hits)} 件の変更（base: {base}）")
    table = ["## Protected paths changed", "", "| path | rule |", "| --- | --- |"]
    for path, pattern in hits:
        if report.on_github():
            print(f"::notice file={path},title=Protected path changed::{path} ({pattern})")
        else:
            print(f"protected path: {path} ({pattern})")
        table.append(f"| `{path}` | `{pattern}` |")
    table += [
        "",
        f"- base: `{base}`",
        "- merge gate: CODEOWNERS (`@yuki-net/reviews`)",
        f"- {GUIDE}",
    ]
    report.write_summary("\n".join(table))
    print(GUIDE)
    return 0


if __name__ == "__main__":
    sys.exit(main())
