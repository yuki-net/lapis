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
    (["scripts/stop_dev_host.ps1", "scripts/test_dev_commands.py", "scripts/test_dev_session.py", "scripts/dev-session/", "scripts/hot-reload/", "scripts/test_hot_reload.py", ".run/"], {"repository", "dev_windows"}),
    (["scripts/check.py", "scripts/test_check.py", ".github/workflows/"], {"repository", "rust_core", "host", "desktop", "mobile", "web", "dev_windows"}),
    # Desktop関連
    (["apps/desktop/client/", "apps/desktop/persistence/"], {"desktop", "rust_core"}),
    (["apps/desktop/"], {"desktop"}),
    (["vendor/gpui/"], {"desktop"}),
    (["patches/gpui-"], {"desktop"}),
    
    # Host関連
    # Desktopが直接利用する公開境界は両surfaceを検証する。
    (["backend/host/Cargo.toml", "backend/host/src/lib.rs", "backend/host/src/client.rs", "backend/host/src/profile.rs"], {"host", "desktop"}),
    (["backend/host/"], {"host"}),
    
    # Backend共通（Host以外）
    (["backend/runtime/"], {"rust_core", "host", "desktop"}),
    (["backend/infrastructure/"], {"rust_core", "host", "desktop"}),
    (["backend/transport/"], {"rust_core", "host", "desktop"}),
    (["backend/application/"], {"rust_core", "host", "desktop"}),
    
    # Shared
    (["shared/models/"], {"rust_core", "host", "desktop"}),
    (["shared/protocol/"], {"rust_core", "host", "desktop"}),
    
    # Mobile
    (["apps/mobile/"], {"mobile"}),
    
    # Web
    (["apps/web/"], {"web"}),
    
    # CI・scripts
    (["scripts/"], {"repository"}),
    
    # Cargo設定
    (["Cargo.toml"], {"repository", "rust_core", "host", "desktop"}),
    (["Cargo.lock"], {"repository", "rust_core", "host", "desktop"}),
    ([".cargo/"], {"repository", "rust_core", "host", "desktop", "dev_windows"}),
    
    # ドキュメント
    (["docs/"], {"repository"}),
    (["locales/"], {"repository", "rust_core", "desktop"}),
]

JOB_IDS = {
    "repository": "repository", "rust_core": "rust_core", "host": "host_windows",
    "desktop": "desktop_windows", "mobile": "mobile", "web": "web",
    "dev_windows": "dev_windows",
}
ALL_JOBS = set(JOB_IDS)


def changed_paths(base_ref: str = "origin/develop") -> list[str]:
    """base_refとの差分で変更されたpathのリストを返す。"""
    try:
        output = subprocess.check_output(
            ["git", "diff", "--no-renames", "--name-only", "--relative", base_ref, "HEAD"],
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
                if path == pattern or (pattern.endswith(("/", "-")) and path.startswith(pattern)):
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
    base_ref = (sys.argv[3] if len(sys.argv) > 3 else "origin/develop") if command == "check" else (sys.argv[2] if len(sys.argv) > 2 else "origin/develop")
    
    paths = changed_paths(base_ref)
    if paths is None:
        # baseが見つからない場合は全jobを実行
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
