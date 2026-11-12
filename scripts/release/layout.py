"""インストール layout と pointer。

layout は後付けできないため、ここで固定する。launcher はこの API だけを使い、
version の探索や保持数を独自に決めない。

    <install root>/
      current.json           現在 version を指す pointer
      versions/<version>/    展開済み package と、その出所の manifest + zip
      staging/               download 済みで未適用の package
      data/<generation>/     DB / data。世代は軸B の release でだけ上げる
      credentials/           data 世代の外。失うと再登録が要る credential
"""

from __future__ import annotations

import json
import os
import shutil
import tempfile
import zipfile
from pathlib import Path

from . import manifest as manifest_module
from .publish import MANIFEST_NAME

POINTER_NAME = "current.json"
POINTER_SCHEMA = 1
VERSIONS_DIR = "versions"
STAGING_DIR = "staging"
DATA_DIR = "data"
CREDENTIALS_DIR = "credentials"
KEEP_VERSIONS = 2
RESERVED_ROOT_NAMES = {MANIFEST_NAME, POINTER_NAME}


class LayoutError(ValueError):
    """layout の破損、または package の展開に失敗。"""


def pointer_path(root: Path) -> Path:
    return root / POINTER_NAME


def versions_dir(root: Path) -> Path:
    return root / VERSIONS_DIR


def version_dir(root: Path, version: str) -> Path:
    _validate_name(version, "version")
    return versions_dir(root) / version


def staging_dir(root: Path) -> Path:
    return root / STAGING_DIR


def data_dir(root: Path, generation: str) -> Path:
    _validate_name(generation, "data generation")
    return root / DATA_DIR / generation


def credentials_dir(root: Path) -> Path:
    """data 世代の外に置く。失うと再登録が要る credential はここだけ。"""
    return root / CREDENTIALS_DIR


def _validate_name(value: str, label: str) -> None:
    if not value or "/" in value or "\\" in value or value in (".", ".."):
        raise LayoutError(f"{label} を directory 名に使えない: {value!r}")


def read_pointer(root: Path) -> str | None:
    """現在 version を返す。pointer が無ければ None、壊れていれば例外。"""
    path = pointer_path(root)
    if not path.exists():
        return None
    try:
        payload = json.loads(path.read_text(encoding="utf-8-sig"))
    except (OSError, json.JSONDecodeError) as error:
        raise LayoutError(f"pointer を読めない: {error}") from error
    if not isinstance(payload, dict) or payload.get("schema") != POINTER_SCHEMA:
        raise LayoutError(f"pointer の schema が不正: {path}")
    if set(payload) != {"schema", "version"}:
        raise LayoutError(f"pointer の field が不正: {path}")
    version = payload["version"]
    if not isinstance(version, str) or not version:
        raise LayoutError(f"pointer の version が不正: {path}")
    return version


def write_pointer(root: Path, version: str) -> None:
    """pointer を原子的に置き換える。書き込み途中の値を見せない。"""
    directory = version_dir(root, version)
    if not directory.is_dir():
        raise LayoutError(f"version directory が無い: {directory}")
    root.mkdir(parents=True, exist_ok=True)
    payload = json.dumps(
        {"schema": POINTER_SCHEMA, "version": version}, sort_keys=True
    ) + "\n"
    handle, temporary = tempfile.mkstemp(dir=root, prefix=".current-", suffix=".tmp")
    try:
        with os.fdopen(handle, "w", encoding="utf-8", newline="\n") as stream:
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, pointer_path(root))
    except BaseException:
        Path(temporary).unlink(missing_ok=True)
        raise


def install(
    root: Path,
    distribution: Path,
    *,
    version: str,
    platform: str,
    trusted_keys: dict[str, str],
    replace: bool = False,
) -> Path:
    """配置済み配布物を検証し、versions/<version>/ へ展開して pointer を進める。

    distribution は manifest と package が同じ directory にある配置（ローカル複写でも
    Releases から download したものでもよい）。生成物と検証経路を分けない。

    既に同名 version がある場合、`replace=False` では失敗する。手動復旧で同じ version を
    入れ直す場合だけ `replace=True` を明示する。置き換えは検証と展開が終わってから行い、
    失敗時は元の directory を残す。
    """
    from . import verify

    verified = verify.verify_distribution(distribution, trusted_keys)
    if verified.version != version:
        raise LayoutError(
            f"manifest の version ({verified.version}) が指定 ({version}) と違う"
        )
    package = verified.packages.get(platform)
    if package is None:
        raise LayoutError(f"platform の package が無い: {platform!r}")
    target = version_dir(root, version)
    if target.exists() and not replace:
        raise LayoutError(
            f"version directory が既にある: {target}。入れ直すなら --replace を使う"
        )
    # pointer が壊れている場合は、展開する前に失敗させる。
    previous = read_pointer(root)
    versions_dir(root).mkdir(parents=True, exist_ok=True)
    staged = Path(tempfile.mkdtemp(dir=versions_dir(root), prefix=".extract-"))
    backup: Path | None = None
    try:
        shutil.copyfile(distribution / MANIFEST_NAME, staged / MANIFEST_NAME)
        shutil.copyfile(package, staged / package.name)
        _extract(package, staged)
        if target.exists():
            backup = Path(tempfile.mkdtemp(dir=versions_dir(root), prefix=".replace-"))
            backup.rmdir()
            os.replace(target, backup)
        try:
            os.replace(staged, target)
        except BaseException:
            if backup is not None:
                os.replace(backup, target)
                backup = None
            raise
    except BaseException:
        shutil.rmtree(staged, ignore_errors=True)
        raise
    if backup is not None:
        shutil.rmtree(backup, ignore_errors=True)
    write_pointer(root, version)
    keep = [version] + ([previous] if previous else [])
    _prune(root, keep=keep[:KEEP_VERSIONS])
    return target


def activate(
    root: Path, version: str, *, platform: str, trusted_keys: dict[str, str]
) -> Path:
    """既に展開済みの version を検証して pointer だけを戻す。

    手動復旧で前版へ戻す経路。package の再取得や再展開をせず、壊れた version の
    directory には触らない。
    """
    from . import verify

    target = version_dir(root, version)
    if not target.is_dir():
        raise LayoutError(f"version directory が無い: {target}")
    verified = verify.verify_install_version(target, trusted_keys, platform=platform)
    if verified.version != version:
        raise LayoutError(
            f"manifest の version ({verified.version}) が指定 ({version}) と違う"
        )
    root.mkdir(parents=True, exist_ok=True)
    write_pointer(root, version)
    return target


def _extract(package: Path, destination: Path) -> None:
    with zipfile.ZipFile(package) as archive:
        for member in archive.infolist():
            name = member.filename
            parts = name.split("/")
            if (
                name.startswith("/")
                or "\\" in name
                or ":" in name
                or any(part in ("", ".", "..") for part in parts)
            ):
                raise LayoutError(f"package に不正な path がある: {name!r}")
            if parts[0] in RESERVED_ROOT_NAMES:
                raise LayoutError(f"package が layout の予約名を使っている: {name!r}")
            if member.is_dir():
                continue
            target = destination.joinpath(*parts)
            target.parent.mkdir(parents=True, exist_ok=True)
            with archive.open(member) as source, target.open("wb") as sink:
                shutil.copyfileobj(source, sink)


def _prune(root: Path, keep: list[str | None]) -> None:
    """keep にない version directory を削除する。

    削除するのは、directory 名と manifest の `version` が一致する「導入済み version」
    だけである。手動復旧で退避した `0.3.0.broken` のような directory は version が
    一致しないため残す。判断できない entry は削除しない。
    """
    retained = {name for name in keep if name}
    directory = versions_dir(root)
    if not directory.is_dir():
        return
    for entry in directory.iterdir():
        if not entry.is_dir() or entry.name in retained:
            continue
        if _installed_version(entry) != entry.name:
            continue
        shutil.rmtree(entry, ignore_errors=True)


def _installed_version(directory: Path) -> str | None:
    """directory が導入済み version なら manifest の version を返す。"""
    path = directory / MANIFEST_NAME
    if not path.is_file():
        return None
    try:
        parsed, _ = manifest_module.parse(path.read_bytes())
    except (ValueError, OSError):
        return None
    return parsed.version
