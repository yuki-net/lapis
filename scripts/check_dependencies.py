"""cargo metadata に基づくクレート間依存のホワイトリスト検査。

workspace member 間の依存辺のうち、scripts/dependency-allowlist.json に無いものだけを
失敗にする。依存の削除や workspace 外への依存は対象にしない。
"""

import json
from pathlib import Path
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[1]
ALLOWLIST = ROOT / "scripts/dependency-allowlist.json"


def load_metadata():
    output = subprocess.check_output(
        ["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"],
        cwd=ROOT,
    )
    return json.loads(output)


def load_allowlist(path=ALLOWLIST):
    data = json.loads(Path(path).read_text(encoding="utf-8"))
    if not isinstance(data, dict):
        raise ValueError("許可リストは crate 名をキーとする object でなければならない")
    allowlist = {}
    for source, targets in data.items():
        if not isinstance(source, str) or not isinstance(targets, list):
            raise ValueError(f"{source}: 許可リストの値は配列でなければならない")
        if not all(isinstance(target, str) for target in targets):
            raise ValueError(f"{source}: 許可リストの要素は crate 名でなければならない")
        allowlist[source] = set(targets)
    return allowlist


def workspace_edges(metadata):
    """workspace member 間の依存辺と、member の package 情報を返す。"""
    members = set(metadata["workspace_members"])
    packages = {
        package["name"]: package
        for package in metadata["packages"]
        if package["id"] in members
    }
    edges = {
        (name, dependency["name"])
        for name, package in packages.items()
        for dependency in package["dependencies"]
        if dependency["name"] in packages
    }
    return packages, edges


def check(metadata, allowlist):
    packages, edges = workspace_edges(metadata)
    allowed = {
        (source, target)
        for source, targets in allowlist.items()
        for target in targets
    }
    errors = []
    for source, target in sorted(edges - allowed):
        manifest = packages[source].get("manifest_path", "")
        location = f" ({manifest})" if manifest else ""
        errors.append(
            f"{source} -> {target}: 依存ホワイトリストに無いクレート間依存{location}"
        )
    return errors


def main():
    try:
        metadata = load_metadata()
        allowlist = load_allowlist()
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"依存ホワイトリストを検査できない: {error}", file=sys.stderr)
        return 1
    errors = check(metadata, allowlist)
    if errors:
        print("\n".join(errors), file=sys.stderr)
        print(
            "scripts/dependency-allowlist.json に、許可する依存辺を追加してください。",
            file=sys.stderr,
        )
        return 1
    _, edges = workspace_edges(metadata)
    print(f"Dependency allowlist OK ({len(edges)} crate edges)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
