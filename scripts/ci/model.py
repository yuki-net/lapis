"""CI の step / route の型定義。

定義の中身は `scripts/ci/routes/` の 1 route 1 ファイルが持ち、ここは
型だけを持つ。型を別ファイルにすることで、route ファイル同士や
`plan.py` / `run.py` が定義ファイルを import し合う循環を作らない。
"""

from dataclasses import dataclass, field

# CI の固定値。ローカルの検証でも同じ値を使う。
RUST_TOOLCHAIN = "1.96.0"
PYTHON_VERSION = "3.12"
NODE_VERSION = "24"
WASM_BINDGEN_VERSION = "0.2.129"
JAVA_VERSION = "21"

# GitHub Actions の runner。route の `runs_on` から引く。
RUNNERS = {"linux": "ubuntu-latest", "windows": "windows-2022"}

# GitHub の job 名に使う OS の表示名（`Host / Windows`）。
OS_LABELS = {"linux": "Linux", "windows": "Windows", "macos": "macOS"}


@dataclass(frozen=True)
class Step:
    """1 つの検証。name は CI の折りたたみ表示とローカルの表に使う。"""

    name: str
    command: list[str]
    # route の作業ディレクトリからの相対パス。既定は route のディレクトリ。
    directory: str | None = None
    # 小数も許す（テストで極小の上限を渡すため）。
    timeout_minutes: float | None = None
    retries: int = 0
    retry_delay_seconds: int = 0
    # 前提を満たさない環境では実行せず unsupported として扱う。
    requires: tuple[str, ...] = ()
    # GitHub では `::error` 注釈を出さず成功扱いにする表示専用の step。
    informational: bool = False
    env: dict[str, str] = field(default_factory=dict)


@dataclass(frozen=True)
class Route:
    """path routing の 1 ルート（= 旧 ci.yml の 1 job）。"""

    key: str
    name: str
    # この route を走らせる OS。CI では集合の分だけ job が生まれる。
    # ローカルは `local_os()` が含まれるかで同じ step を実行できるか判断する。
    runs_on: tuple[str, ...]
    timeout_minutes: int
    steps: tuple[Step, ...]
    # runner の OS ではなく検証対象を job 名に出したいときのラベル。
    # 例: Mobile は Linux runner で Android を検証するので "Android"。
    # 複数 OS で走る route は job 名の重複を避けるため OS を優先する。
    target: str = ""

    # checkout の fetch-depth。0 は全履歴。
    checkout_depth: int = 1
    python: str = ""
    node: str = ""
    node_cache: str = ""
    node_cache_path: str = ""
    rust_components: str = ""
    rust_targets: str = ""
    rust: bool = False
    wasm_bindgen: str = ""
    java: str = ""
    gradle: bool = False
    # actions/cache の key 接頭辞。空ならキャッシュしない。
    cache: str = ""
    # キャッシュする target-dir（リポジトリルート相対）。
    cache_paths: tuple[str, ...] = ("target/",)
    # route の既定作業ディレクトリ（リポジトリルートからの相対パス）。
    working_directory: str = ""
