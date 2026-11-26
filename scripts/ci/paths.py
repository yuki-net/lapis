"""変更 path から実行すべき route を決める。

ルーティングは「route 同士の関係」なので、route ごとの定義
（`scripts/ci/routes/`）ではなく 1 箇所にまとめる。1 つの path が
複数の route を要求する（例: `host/` 配下は rust_core / host / desktop）
ため、`PATH_RULES` は パターン群 → route 集合 の表として持つ。

判定は次に一致した規則を採用する（上から順に見て、そこで打ち切る）。
そのため狭い規則を先に並べる。どの規則にも当たらない path は、安全側に
倒して全 route を実行する。
"""

import subprocess

from scripts.ci.model import ROOT
from scripts.ci.routes import ORDER, ROUTES

# どの規則にも当たらない変更で走らせる route の集合。
ALL_JOBS = frozenset(ROUTES)

# path パターン → 実行すべき route の集合。優先度順（狭いパターンを先に）。
PATH_RULES = [
    (["scripts/stop_dev_host.ps1", "scripts/test_dev_commands.py", "scripts/test_dev_session.py",
      "scripts/dev-session/", ".run/"], {"repository", "dev_commands"}),
    # CI の定義そのものと、CI を動かすスクリプト。どの route が壊れたか
    # 分からないので全 route を回す。
    (["scripts/check.py", "scripts/test_check.py", "scripts/ci/", ".github/workflows/"],
     set(ALL_JOBS)),
    # Desktop 関連。
    # clients/state と clients/ui は Web の wasm ビルド（apps/web/wasm）でも
    # コンパイルされる。
    (["clients/state/"], {"desktop", "rust_core", "web"}),
    (["apps/desktop/persistence/"], {"desktop", "rust_core"}),
    (["apps/launcher/"], {"desktop", "rust_core"}),
    (["apps/desktop/"], {"desktop"}),
    (["clients/ui/"], {"desktop", "web"}),
    (["workbench/"], {"rust_core", "desktop", "web"}),
    (["tools/devtools/"], {"desktop"}),
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
    (["host/transport/"], {"rust_core", "host", "desktop"}),
    (["host/application/"], {"rust_core", "host", "desktop"}),
    # 上記より狭い規則に当たらない Host 配下（削除された crate の path を
    # 含む）。未分類のままにすると全 job が走るため、Host の面へ割り当てる。
    (["host/"], {"rust_core", "host", "desktop"}),

    # Protocol / Client transport。
    # 領域の README はコード変更ではないため repository だけで検証する。
    (["protocol/README.md", "protocol/extension-api/README.md"], {"repository"}),
    (["protocol/extension-api/native/"], {"rust_core", "host", "desktop"}),
    (["protocol/extension-api/"], {"rust_core", "host", "desktop", "web"}),
    (["protocol/types/"], {"rust_core", "host", "desktop", "web"}),
    (["protocol/wire/client-api/"], {"rust_core", "host", "desktop", "web"}),
    (["protocol/wire/dto/"], {"rust_core", "host", "desktop", "web"}),
    (["protocol/wire-client/"], {"rust_core", "host", "desktop", "web"}),
    # 移設前の path は削除差分として現れるため、全 route を起こさず元の面へ割り当てる。
    (["contracts/extension-api/native/"], {"rust_core", "host", "desktop"}),
    (["contracts/"], {"rust_core", "host", "desktop", "web"}),
    (["clients/transport/"], {"rust_core", "host", "desktop", "web"}),

    # Extensions。
    # 拡張の contract は Client からも参照するため両 surface を検証する。
    # Terminal 拡張は役割ごとに影響する surface が異なるため、契約と実装を
    # 分ける（実装は PTY を含み wasm でビルドしない）。
    (["extensions/terminal/contract/"], {"rust_core", "host", "desktop", "web"}),
    (["extensions/terminal/adapter/", "extensions/terminal/README.md"],
     {"rust_core", "host", "desktop"}),
    # Git / ACP の Host 実装は adapter（infrastructure）として extensions 配下へ
    # 移した。契約と異なり Client の wasm ではビルドしない。
    (["extensions/git/adapter/", "extensions/acp/adapter/"],
     {"rust_core", "host", "desktop"}),
    # 未登録の拡張・宣言・アセットも Client へ影響し得るため、既知の Host
    # 専用箇所以外は広く検証する。
    (["extensions/"], {"repository", "rust_core", "host", "desktop", "web"}),

    # Mobile。
    (["clients/mobile/"], {"mobile"}),

    # Web。
    # 移設前の path は削除差分として現れるため、全 job を起こさず Web の検証へ割り当てる。
    (["clients/web/"], {"web"}),
    (["apps/web/"], {"web"}),

    # CI・scripts。
    # package 生成と Host の読み込みを同じ fixture で検証する。
    (["scripts/extensions/"], {"repository", "rust_core", "host"}),
    (["scripts/"], {"repository"}),
    # エージェント用の設定・スキルはコードではない。
    ([".opencode/"], {"repository"}),
    # launcher は release/trusted-keys.json を build 時に埋め込む。
    (["release/"], {"repository", "desktop"}),

    # Cargo 設定。
    (["Cargo.toml"], {"repository", "rust_core", "host", "desktop", "web"}),
    (["Cargo.lock"], {"repository", "rust_core", "host", "desktop", "web"}),
    ([".cargo/"], {"repository", "rust_core", "host", "desktop", "dev_commands"}),

    # lint 基盤。カタログ・fixture・baseline の検査は repository が持つ。
    (["lints/"], {"repository"}),

    # サイト。サイトごとに route を分け、触っていないサイトは走らせない。
    (["sites/docs/"], {"sites_docs"}),
    (["sites/"], {"sites_docs"}),

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


def _git(*args: str) -> str | None:
    """git を実行して標準出力を返す。失敗したら None。"""
    try:
        return subprocess.check_output(
            ["git", *args],
            cwd=ROOT,
            text=True,
            encoding="utf-8",
            # git の改行警告などを判定結果に混ぜない。
            stderr=subprocess.DEVNULL,
        )
    except (subprocess.CalledProcessError, FileNotFoundError):
        return None


def _lines(output: str) -> list[str]:
    return [line for line in output.strip().splitlines() if line]


def pr_paths(base_ref: str) -> list[str] | None:
    """PR が base から持ち込む変更。

    `A...B`（three-dot）は merge-base からの差分になる。base 側にしか無い
    コミットを「削除」と誤認しないため、PR の差分はこちらで見る。base が
    HEAD の祖先なら結果は `A..B` と同じ。解決できなければ None。
    """
    output = _git("diff", "--no-renames", "--name-only", "--relative", f"{base_ref}...HEAD")
    return None if output is None else _lines(output)


def worktree_paths() -> list[str] | None:
    """まだコミットしていない変更（staged + unstaged + untracked）。

    `git diff HEAD` は untracked を含まないので、`ls-files --others` で足す。
    新規ディレクトリを足したときに route を取りこぼさないため。
    """
    tracked = _git("diff", "--no-renames", "--name-only", "--relative", "HEAD")
    untracked = _git("ls-files", "--others", "--exclude-standard")
    if tracked is None or untracked is None:
        return None
    return sorted(set(_lines(tracked)) | set(_lines(untracked)))


def changed_paths(base_ref: str = "origin/develop", include_worktree: bool = True) -> list[str] | None:
    """変更された path のリストを返す。

    - `include_worktree=True`: PR の差分と、まだコミットしていない変更の和。
      手元では「PR 全体＋いま触っている所」、CI では作業ツリーが clean なので
      PR の差分だけになる。
    - base が解決できない場合（浅い clone、CI 外など）は None を返し、
      呼び出し側が「全 route を実行」に倒せるようにする。
    """
    committed = pr_paths(base_ref)
    if committed is None:
        return None
    if not include_worktree:
        return committed
    pending = worktree_paths()
    if pending is None:
        return committed
    return sorted(set(committed) | set(pending))
