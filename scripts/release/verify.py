"""配布物とインストール layout の検証。

client 側の更新処理はこの検証をそのまま使う。配置の入り口（ローカル / Releases）で
検証項目を変えない。
"""

from __future__ import annotations

import hashlib
import zipfile
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
    """展開済み version directory の manifest・package・展開内容を確認する。

    保存済み ZIP だけでは、展開後の実行 file を書き換えた version を検出できない。
    展開済み file も ZIP の内容と照合し、一致する platform が 1 つも無ければ失敗する。
    """
    verified = verify_distribution(directory, trusted_keys)
    if platform is not None:
        package = verified.packages.get(platform)
        if package is None:
            raise layout.LayoutError(f"platform の package が無い: {platform!r}")
        candidates = {platform: package}
    else:
        candidates = dict(verified.packages)
    skip = {MANIFEST_NAME, layout.POINTER_NAME} | {
        path.name for path in verified.packages.values()
    }
    failures = {}
    for name, package in sorted(candidates.items()):
        mismatched = extracted_mismatches(directory, package, skip=skip)
        if not mismatched:
            return verified
        failures[name] = mismatched
    detail = "; ".join(
        f"{name}: {', '.join(items[:3])}" for name, items in sorted(failures.items())
    )
    raise layout.LayoutError(f"展開済み file が package と一致しない: {detail}")


def extracted_mismatches(
    directory: Path, package: Path, *, skip: set[str] = frozenset()
) -> list[str]:
    """展開済み file を package の内容と照合し、不一致の説明を返す。

    余分な file も欠けた file も不一致とする。`skip` には ZIP 自身や manifest など、
    展開内容に含まれない file 名を渡す。
    """
    expected = {}
    with zipfile.ZipFile(package) as archive:
        for member in archive.infolist():
            name = layout.member_name(member)
            if member.is_dir():
                continue
            digest = hashlib.sha256()
            with archive.open(member) as stream:
                for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                    digest.update(chunk)
            expected[name] = digest.hexdigest()
    actual = {}
    for path in directory.rglob("*"):
        relative = path.relative_to(directory).as_posix()
        if relative in skip:
            continue
        # symlink は参照先を辿らず不一致とする。展開物は通常 file だけであり、
        # symlink を許すと install 外の file を実行対象にできてしまう。
        if path.is_symlink():
            actual[relative] = None
            continue
        if path.is_dir():
            continue
        digest = hashlib.sha256()
        with path.open("rb") as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                digest.update(chunk)
        actual[relative] = digest.hexdigest()
    problems = []
    for name in sorted(set(expected) | set(actual)):
        if name not in actual:
            problems.append(f"{name} が無い")
        elif name not in expected:
            problems.append(f"{name} が package に無い")
        elif actual[name] is None:
            problems.append(f"{name} が symlink")
        elif expected[name] != actual[name]:
            problems.append(f"{name} が package と違う")
    return problems
