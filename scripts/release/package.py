"""配布 package (zip) の生成。

同じ入力から同じ bytes を作る。配置先や検証の形式をここで分岐させない。
"""

from __future__ import annotations

import os
import re
import stat
import zipfile
from pathlib import Path

FIXED_TIMESTAMP = (1980, 1, 1, 0, 0, 0)
EXECUTABLE_SUFFIXES = {".exe", ".cmd", ".bat", ".sh", ""}
NAME_PATTERN = re.compile(r"^[a-z0-9]+([._-][a-z0-9]+)*$")


class PackageError(ValueError):
    """package に含められない入力。"""


def package_name(name: str, version: str, platform: str) -> str:
    """配置先で衝突しない package 名を作る。名前に path 要素を許さない。"""
    if not NAME_PATTERN.match(name):
        raise PackageError(f"package 名の形式が不正: {name!r}")
    if not NAME_PATTERN.match(platform):
        raise PackageError(f"platform 名の形式が不正: {platform!r}")
    if not version or "/" in version or "\\" in version or ".." in version:
        raise PackageError(f"version の形式が不正: {version!r}")
    return f"{name}-{version}-{platform}.zip"


def collect(input_dir: Path) -> list[Path]:
    """input_dir 配下の通常 file を相対パス順で返す。"""
    if not input_dir.is_dir():
        raise PackageError(f"入力 directory が無い: {input_dir}")
    files = []
    for path in input_dir.rglob("*"):
        if path.is_symlink():
            raise PackageError(f"symlink は package に含めない: {path}")
        if path.is_file():
            files.append(path)
    if not files:
        raise PackageError(f"入力 directory が空: {input_dir}")
    return sorted(files, key=lambda path: path.relative_to(input_dir).as_posix())


def build(input_dir: Path, output: Path) -> Path:
    """input_dir の中身を zip の root に置いた package を書く。"""
    files = collect(input_dir)
    output.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for path in files:
            name = path.relative_to(input_dir).as_posix()
            info = zipfile.ZipInfo(name, date_time=FIXED_TIMESTAMP)
            info.create_system = 3
            mode = 0o755 if os.name != "nt" and path.suffix in EXECUTABLE_SUFFIXES else 0o644
            info.external_attr = (stat.S_IFREG | mode) << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, path.read_bytes(), compresslevel=9)
    return output
