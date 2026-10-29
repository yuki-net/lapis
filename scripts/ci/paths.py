"""CIのpath routingロジック。YAMLではなくテスト可能なscriptで管理する。

各jobがどのpath変更で実行されるかを判定し、未知のpathでは安全側に倒す。
"""

from pathlib import Path
import json
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]

# pathパターン → 実行すべきjobのセット
# 優先度順：狭いパターンを先に定義
PATH_RULES = [
    # Desktop関連
    (["apps/desktop/"], {"desktop"}),
    (["vendor/gpui/"], {"desktop"}),
    (["patches/gpui-"], {"desktop"}),
    
    # Host関連
    (["backend/host/"], {"host"}),
    
    # Backend共通（Host以外）
    (["backend/runtime/"], {"rust_core"}),
    (["backend/infrastructure/"], {"rust_core", "host"}),
    (["backend/transport/"], {"rust_core", "host"}),
    (["backend/application/"], {"rust_core"}),
    
    # Shared
    (["shared/models/"], {"rust_core"}),
    (["shared/protocol/"], {"rust_core"}),
    
    # Mobile
    (["apps/mobile/"], {"mobile"}),
    
    # Web
    (["apps/web/"], {"web"}),
    
    # CI・scripts
    ([".github/workflows/"], {"repository"}),
    (["scripts/"], {"repository"}),
    
    # Cargo設定
    (["Cargo.toml"], {"repository", "rust_core", "host", "desktop"}),
    (["Cargo.lock"], {"repository", "rust_core", "host", "desktop"}),
    ([".cargo/"], {"repository", "rust_core", "host", "desktop"}),
    
    # ドキュメント
    (["docs/"], {"repository"}),
    (["locales/"], {"repository"}),
]

ALL_JOBS = {"repository", "rust_core", "host", "desktop", "mobile", "web"}


def changed_paths(base_ref: str = "origin/develop") -> list[str]:
    """base_refとの差分で変更されたpathのリストを返す。"""
    try:
        output = subprocess.check_output(
            ["git", "diff", "--name-only", "--relative", f"{base_ref}..."],
            cwd=ROOT,
            text=True,
        )
        return [line for line in output.strip().splitlines() if line]
    except subprocess.CalledProcessError:
        # baseが見つからない場合（CI外など）、全jobを実行
        return None


def jobs_for_paths(paths: list[str]) -> set[str]:
    """変更されたpathから実行すべきjobのセットを返す。"""
    if not paths:
        return set()
    
    jobs = set()
    for path in paths:
        matched = False
        for patterns, job_set in PATH_RULES:
            for pattern in patterns:
                if path.startswith(pattern) or path == pattern:
                    jobs.update(job_set)
                    matched = True
                    break
            if matched:
                break
        # 未知のpathは安全側に倒して全jobを実行
        if not matched:
            return ALL_JOBS | {"repository"}
    
    return jobs


def should_run(job: str, paths: list[str], force_all: bool = False) -> bool:
    """jobを実行するかどうかを判定する。"""
    if force_all:
        return True
    return job in jobs_for_paths(paths)


def main() -> int:
    """CLI入口。CIから呼び出される。
    
    Usage:
        python scripts/ci/paths.py list          # 実行すべきjobをJSON配列で出力
        python scripts/ci/paths.py check <job>    # jobを実行する場合は0、skipは1
    """
    if len(sys.argv) < 2:
        print("Usage: paths.py [list|check <job>]", file=sys.stderr)
        return 1
    
    command = sys.argv[1]
    base_ref = sys.argv[2] if len(sys.argv) > 2 else "origin/develop"
    
    paths = changed_paths(base_ref)
    if paths is None:
        # baseが見つからない場合は全jobを実行
        paths = list(ROOT.rglob("*"))  # ダミー：実際にはALL_JOBSを返す
        jobs = ALL_JOBS | {"repository"}
    else:
        jobs = jobs_for_paths(paths)
    
    if command == "list":
        print(json.dumps(sorted(jobs)))
        return 0
    elif command == "check":
        if len(sys.argv) < 3:
            print("Usage: paths.py check <job>", file=sys.stderr)
            return 1
        job = sys.argv[2]
        if job in jobs:
            return 0
        return 1
    else:
        print(f"Unknown command: {command}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
