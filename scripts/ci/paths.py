"""変更 path から実行すべき route を決める。

ルーティングは「route 同士の関係」なので、route ごとの定義
（`scripts/ci/routes/`）ではなく 1 箇所にまとめる。1 つの path が
複数の route を要求する（例: `host/` 配下は rust_core / host / desktop）
ため、`PATH_RULES` は パターン群 → route 集合 の表として持つ。

判定は次に一致した規則を採用する（上から順に見て、そこで打ち切る）。
そのため狭い規則を先に並べる。どの規則にも当たらない path は、安全側に
倒して全 route を実行する。
"""

from pathlib import Path
import subprocess

from scripts.ci.routes import ORDER, ROUTES

ROOT = Path(__file__).resolve().parents[2]

# どの規則にも当たらない変更で走らせる route の集合。
ALL_JOBS = frozenset(ROUTES)

# path パターン → 実行すべき route の集合。優先度順（狭いパターンを先に）。
PATH_RULES = [
    (["scripts/stop_dev_host.ps1", "scripts/test_dev_commands.py", "scripts/test_dev_session.py",
      "scripts/dev-session/", ".run/"], {"repository", "dev_commands"}),
    (["scripts/check.py", "scripts/test_check.py", ".github/workflows/"],
     {"repository", "rust_core", "host", "desktop", "mobile", "web", "dev_commands"}),
    # Desktop 関連。
    # clients/state と clients/ui は Web の wasm ビルド（desktop-ui）でも
    # コンパイルされる。
    (["clients/state/src/web_host.rs", "clients/state/src/web_host/extension_terminal.rs",
      "clients/state/src/terminal_extension/", "clients/state/src/terminal_extension.rs",
      "clients/ui/src/web/mod.rs", "clients/ui/wasm-smoke/",
      "clients/web/wasm/tests/extension_terminal_smoke.py"], {"host", "desktop", "rust_core", "web"}),
    (["clients/state/"], {"desktop", "rust_core", "web"}),
    (["clients/desktop/persistence/"], {"desktop", "rust_core"}),
    (["clients/desktop/launcher/"], {"desktop", "rust_core"}),
    (["clients/desktop/"], {"desktop"}),
    (["clients/ui/"], {"desktop", "web"}),
    (["vendor/gpui/"], {"desktop"}),
    (["patches/gpui-"], {"desktop"}),

    # Host 関連。
    # 領域の README はコード変更ではないため repository だけで検証する
    # （未分類のままだと全 job が走る）。
    (["host/README.md"], {"repository"}),
    # Desktop が直接利用する公開境界は両 surface を検証する。
    (["host/app/Cargo.toml", "host/app/src/lib.rs", "host/app/src/client.rs", "host/app/src/profile.rs"],
     {"host", "desktop"}),
    (["host/app/"], {"host"}),

    # Backend 共通（Host 以外）。
    (["host/extensions/"], {"rust_core", "host", "desktop"}),
    (["host/extensions-entry/"], {"rust_core", "host", "desktop"}),
    (["host/runtime/"], {"rust_core", "host", "desktop"}),
    (["host/adapters/"], {"rust_core", "host", "desktop"}),
    (["host/transport/web/"], {"rust_core", "host", "desktop", "web"}),
    (["host/transport/web-bridge/"], {"host", "desktop", "web"}),
    (["host/transport/"], {"rust_core", "host", "desktop"}),
    (["host/application/"], {"rust_core", "host", "desktop"}),
    # 上記より狭い規則に当たらない Host 配下（削除された crate の path を
    # 含む）。未分類のままにすると全 job が走るため、Host の面へ割り当てる。
    (["host/"], {"rust_core", "host", "desktop"}),

    # Contracts / Client transport。
    (["contracts/extension-api/native/"], {"rust_core", "host", "desktop"}),
    (["contracts/domain/"], {"rust_core", "host", "desktop", "web"}),
    (["contracts/protocol/"], {"rust_core", "host", "desktop", "web"}),
    (["contracts/transport/wire/"], {"rust_core", "host", "desktop", "web"}),
    (["clients/transport/"], {"rust_core", "host", "desktop", "web"}),

    # Extensions。
    # 拡張の contract は Client からも参照するため両 surface を検証する。
    # Terminal 拡張は役割ごとに影響する surface が異なるため、契約と実装を
    # 分ける（実装は PTY を含み wasm でビルドしない）。
    (["extensions/terminal/contract/"], {"rust_core", "host", "desktop", "web"}),
    (["extensions/terminal/adapter/", "extensions/terminal/README.md"],
     {"rust_core", "host", "desktop"}),
    # 未登録の拡張・宣言・アセットも Client へ影響し得るため、既知の Host
    # 専用箇所以外は広く検証する。
    (["extensions/"], {"repository", "rust_core", "host", "desktop", "web"}),

    # Mobile。
    (["clients/mobile/"], {"mobile"}),

    # Web。
    (["clients/web/"], {"web"}),

    # CI・scripts。
    # package 生成と Host の読み込みを同じ fixture で検証する。
    (["scripts/extensions/"], {"repository", "rust_core", "host"}),
    (["scripts/"], {"repository"}),
    # launcher は release/trusted-keys.json を build 時に埋め込む。
    (["release/"], {"repository", "desktop"}),

    # Cargo 設定。
    (["Cargo.toml"], {"repository", "rust_core", "host", "desktop"}),
    (["Cargo.lock"], {"repository", "rust_core", "host", "desktop"}),
    ([".cargo/"], {"repository", "rust_core", "host", "desktop", "dev_commands"}),

    # ドキュメント。
    (["docs/"], {"repository"}),
    (["locales/"], {"repository", "rust_core", "desktop"}),
]


def matches(path: str, pattern: str) -> bool:
    """path が規則のパターンに当たるか。"""
    return path == pattern or (pattern.endswith(("/", "-")) and path.startswith(pattern))


def jobs_for_paths(paths: list[str]) -> set[str]:
    """変更された path から実行すべき route のセットを返す。"""
    if not paths:
        return set()

    jobs: set[str] = set()
    for path in paths:
        for patterns, route_set in PATH_RULES:
            if any(matches(path, pattern) for pattern in patterns):
                jobs.update(route_set)
                break
        else:
            # 未知の path は安全側に倒して全 route を実行。
            return set(ALL_JOBS)
    return jobs


def should_run(job: str, paths: list[str], force_all: bool = False) -> bool:
    """job を実行するかどうかを判定する。"""
    if force_all:
        return True
    return job in jobs_for_paths(paths)


def changed_paths(base_ref: str = "origin/develop") -> list[str] | None:
    """base_ref との差分で変更された path のリストを返す。

    base が解決できない場合（CI 外など）は None を返し、呼び出し側が
    「全 route を実行」に倒せるようにする。
    """
    try:
        output = subprocess.check_output(
            ["git", "diff", "--no-renames", "--name-only", "--relative", base_ref, "HEAD"],
            cwd=ROOT,
            text=True,
            encoding="utf-8",
        )
    except subprocess.CalledProcessError:
        return None
    return [line for line in output.strip().splitlines() if line]
