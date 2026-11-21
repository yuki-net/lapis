"""`scripts/ci/model.py` と `scripts/ci/routes/` の公開面。

route の検証内容は `scripts/ci/routes/<key>.py` が 1 route 1 ファイルで
持ち、どの変更 path でどの route を走らせるかは route 同士の関係なので
`scripts/ci/paths.py` が持つ。この module は従来の import 先
（`scripts.ci.jobs`）を保つための再エクスポートに彻する。
"""

import os
import sys
from pathlib import Path

from scripts.ci.model import (
    JAVA_VERSION,
    NODE_VERSION,
    OS_LABELS,
    PYTHON_VERSION,
    RUNNERS,
    RUST_TOOLCHAIN,
    WASM_BINDGEN_VERSION,
    Route,
    Step,
)
from scripts.ci.routes import ORDER, ROUTES, job_name, route_for

ROOT = Path(__file__).resolve().parents[2]


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
