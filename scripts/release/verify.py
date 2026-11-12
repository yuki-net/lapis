"""配布物とインストール layout の検証。

client 側の更新処理はこの検証をそのまま使う。配置の入り口（ローカル / Releases）で
検証項目を変えない。
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

from . import layout, manifest as manifest_module
from .publish import MANIFEST_NAME, PublishError


@dataclass(frozen=True)
class Verified:
    version: str
    breaking: bool
    notes_url: str
    packages: dict[str, Path]


def verify_distribution(directory: Path, trusted_keys: dict[str, str]) -> Verified:
    """manifest の署名と全 package の SHA-256 を確認する。"""
    manifest_path = directory / MANIFEST_NAME
    if not manifest_path.is_file():
        raise PublishError(f"manifest が無い: {manifest_path}")
    raw = manifest_path.read_bytes()
    parsed, payload = manifest_module.parse(raw)
    manifest_module.verify_signature(payload, trusted_keys)
    packages = {}
    for platform, asset in parsed.assets.items():
        packages[platform] = manifest_module.verify_asset(
            parsed, payload, platform, directory
        )
    return Verified(
        version=parsed.version,
        breaking=parsed.breaking,
        notes_url=parsed.notes_url,
        packages=packages,
    )


def verify_install(root: Path, trusted_keys: dict[str, str]) -> Verified:
    """インストール済み layout の pointer と version の対応を確認する。"""
    current = layout.read_pointer(root)
    if current is None:
        raise layout.LayoutError(f"pointer が無い: {layout.pointer_path(root)}")
    directory = layout.version_dir(root, current)
    if not directory.is_dir():
        raise layout.LayoutError(f"pointer の version が無い: {directory}")
    verified = verify_install_version(directory, trusted_keys)
    if verified.version != current:
        raise layout.LayoutError(
            f"pointer ({current}) と manifest ({verified.version}) が一致しない"
        )
    return verified


def verify_install_version(
    directory: Path, trusted_keys: dict[str, str], *, platform: str | None = None
) -> Verified:
    """展開済み version directory の manifest と package を確認する。"""
    verified = verify_distribution(directory, trusted_keys)
    if platform is not None and platform not in verified.packages:
        raise layout.LayoutError(f"platform の package が無い: {platform!r}")
    return verified

