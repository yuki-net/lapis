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
    (["scripts/stop_dev_host.ps1", "scripts/test_dev_commands.py", "scripts/test_dev_session.py", "scripts/dev-session/", ".run/"], {"repository", "dev_windows"}),
    (["scripts/check.py", "scripts/test_check.py", ".github/workflows/"], {"repository", "rust_core", "host", "desktop", "mobile", "web", "dev_windows"}),
    # Desktop関連
    # clients/state と clients/ui は Web の wasm ビルド（desktop-ui）でもコンパイルされる。
    (["clients/state/"], {"desktop", "rust_core", "web"}),
    (["clients/desktop/persistence/"], {"desktop", "rust_core"}),
    (["clients/desktop/launcher/"], {"desktop", "rust_core"}),
    (["clients/desktop/"], {"desktop"}),
    (["clients/ui/"], {"desktop", "web"}),
    (["vendor/gpui/"], {"desktop"}),
    (["patches/gpui-"], {"desktop"}),
    
    # Host関連
    # Desktopが直接利用する公開境界は両surfaceを検証する。
    (["host/app/Cargo.toml", "host/app/src/lib.rs", "host/app/src/client.rs", "host/app/src/profile.rs"], {"host", "desktop"}),
    (["host/app/"], {"host"}),
    
    # Backend共通（Host以外）
    (["host/extensions/"], {"rust_core", "host", "desktop"}),
    (["host/runtime/"], {"rust_core", "host", "desktop"}),    (["host/adapters/"], {"rust_core", "host", "desktop"}),
    (["host/transport/web/"], {"rust_core", "host", "desktop", "web"}),
    (["host/transport/web-bridge/"], {"host", "desktop", "web"}),
    (["host/transport/"], {"rust_core", "host", "desktop"}),
    (["host/application/"], {"rust_core", "host", "desktop"}),
    
    # Contracts / Client transport
    (["contracts/domain/"], {"rust_core", "host", "desktop", "web"}),
    (["contracts/protocol/"], {"rust_core", "host", "desktop", "web"}),
    (["contracts/transport/wire/"], {"rust_core", "host", "desktop", "web"}),
    (["clients/transport/"], {"rust_core", "host", "desktop", "web"}),

    # Extensions
    # 拡張の contract は Client からも参照するため両surfaceを検証する。
    # Terminal 拡張は役割ごとに影響する surface が異なるため、契約と実装を分ける。
    # （実装は PTY を含み wasm でビルドしない）
    (["extensions/terminal/contract/"], {"rust_core", "host", "desktop", "web"}),
    (["extensions/terminal/"], {"rust_core", "host", "desktop"}),
    (["extensions/"], {"rust_core", "host", "desktop", "web"}),
    
    # Mobile
    (["clients/mobile/"], {"mobile"}),
    
    # Web
    (["clients/web/"], {"web"}),
    
    # CI・scripts
    (["scripts/"], {"repository"}),
    # launcher は release/trusted-keys.json を build 時に埋め込む。
    (["release/"], {"repository", "desktop"}),
    
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
