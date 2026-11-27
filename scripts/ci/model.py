"""CI の step / route の型定義。

定義の中身は `scripts/ci/routes/` の 1 route 1 ファイルが持ち、ここは
型だけを持つ。型を別ファイルにすることで、route ファイル同士や
`plan.py` / `run.py` が定義ファイルを import し合う循環を作らない。

OS ごとに step が違う場合の差分は `scripts/ci/routes/platforms/` に
`<route>_<os>.py`（`STEPS` を定義）を置く。ファイルを置くだけで
`Route.platform_steps` に載るので、route ファイルを書き換えなくてよい。
"""

import importlib.util
import os
import sys
from dataclasses import dataclass, field
from pathlib import Path

# リポジトリルート。git の作業ディレクトリやログの置き場に使う。
ROOT = Path(__file__).resolve().parents[2]

# CI の固定値。ローカルの検証でも同じ値を使う。
RUST_TOOLCHAIN = "1.96.0"
PYTHON_VERSION = "3.12"
NODE_VERSION = "24"
WASM_BINDGEN_VERSION = "0.2.129"
JAVA_VERSION = "21"

# GitHub Actions の runner。route の `runs_on` から引く。
RUNNERS = {"linux": "ubuntu-latest", "windows": "windows-2022"}

# GitHub の job 名に使う OS の表示名（`Host / Windows`）。
OS_LABELS = {"linux": "Linux", "windows": "Windows"}

# OS ごとの追加 step を置くディレクトリ（`<route>_<os>.py`）。
PLATFORM_DIRECTORY = Path(__file__).resolve().parent / "routes" / "platforms"


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
    # 変更 path がこのパターンのいずれかに一致するときだけ実行する。
    # 空なら常に実行する（path で絞らない）。
    paths: tuple[str, ...] = ()
    env: dict[str, str] = field(default_factory=dict)
    # cargo の `--message-format=json` 出力を診断として解釈する。成否は診断の
    # 有無で決め、GitHub の注釈とジョブサマリに変換する（collect-then-fail）。
    json_diagnostics: bool = False


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
    # `--prefer-os` の対象外にする（lint のように全 OS で見たい route）。
    always_all_os: bool = False
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
    # actions/cache ではなく rust-cache を使うか（Rust のビルドを含む route）。
    cache: bool = False
    # route の既定作業ディレクトリ（リポジトリルートからの相対パス）。
    working_directory: str = ""

    @property
    def platform_steps(self) -> dict[str, tuple["Step", ...]]:
        """OS ごとの追加 step。`routes/platforms/<key>_<os>.py` から拾う。

        ファイルが無い OS は含まれない（共通 step だけで走る）。route
        ファイル側に列挙を持たせないので、差分を足すときはファイルを
        置くだけでよい。
        """
        return platform_steps(self.key)


def platform_steps(route_key: str) -> dict[str, tuple[Step, ...]]:
    """`PLATFORM_DIRECTORY` から route の OS 別 step を集める。"""
    return load_platform_steps(PLATFORM_DIRECTORY, route_key)


def load_platform_steps(directory: Path, route_key: str) -> dict[str, tuple[Step, ...]]:
    """`<route_key>_<os>.py` を読み、OS 名 → step 列 にする。

    ファイル名の OS が `RUNNERS` に無い場合は、綴り間違いをそのまま
    無視しないよう例外にする。
    """
    found: dict[str, tuple[Step, ...]] = {}
    prefix = f"{route_key}_"
    for path in sorted(Path(directory).glob(f"{prefix}*.py")):
        os_name = path.stem[len(prefix):]
        if os_name not in RUNNERS:
            raise ValueError(
                f"{path.name}: unknown OS '{os_name}' (expected {', '.join(RUNNERS)})"
            )
        module = _load_module(path)
        steps = getattr(module, "STEPS", None)
        if steps is None:
            raise ValueError(f"{path.name}: STEPS を定義すること")
        found[os_name] = tuple(steps)
    return found


def _load_module(path: Path):
    spec = importlib.util.spec_from_file_location(f"lapis_ci_platform_{path.stem}", path)
    if spec is None or spec.loader is None:
        raise ValueError(f"{path.name}: 読み込めない")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def requirements(route: Route) -> tuple[str, ...]:
    """route が必要とする前提。run.py がローカルで充足を判定する。"""
    required = []
    if route.python:
        required.append("python")
    if route.rust:
        required.append("cargo")
    if route.node:
        required.append("node")
    if route.wasm_bindgen:
        required.append("wasm-bindgen")
    if route.java:
        required.append("java")
    if route.gradle:
        required.append("gradle")
    return tuple(required)


def local_os() -> str:
    return "windows" if os.name == "nt" else "linux" if sys.platform.startswith("linux") else "macos"
