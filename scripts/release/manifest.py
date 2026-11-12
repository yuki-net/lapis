"""manifest の生成と検証。

schema v1 は初日から固定する。未知の field は拒否する。項目を後から足すと
旧 client が解釈できないため、追加は schema の改訂として扱う。
"""

from __future__ import annotations

import base64
import hashlib
import json
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from . import ed25519

SCHEMA = 1
ASSET_FIELDS = {"source", "sha256"}
MANIFEST_FIELDS = {
    "schema",
    "version",
    "breaking",
    "notes_url",
    "generated_at",
    "key_id",
    "assets",
    "signature",
}

VERSION_PATTERN = re.compile(r"^[0-9]+(\.[0-9]+)*([.-][0-9A-Za-z]+)*$")
SHA256_PATTERN = re.compile(r"^[0-9a-f]{64}$")
PLATFORM_PATTERN = re.compile(r"^[a-z0-9]+([._-][a-z0-9]+)*$")


class ManifestError(ValueError):
    """manifest の形式・署名・asset が不正。"""


def canonical_bytes(value: Any) -> bytes:
    """署名対象の正規化 bytes。key 順と区切りを固定する。"""
    return json.dumps(
        value, sort_keys=True, separators=(",", ":"), ensure_ascii=False
    ).encode("utf-8")


def signable_payload(manifest: dict[str, Any]) -> dict[str, Any]:
    return {key: value for key, value in manifest.items() if key != "signature"}


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def validate_version(version: str) -> None:
    if not VERSION_PATTERN.match(version) or ".." in version:
        raise ManifestError(f"version の形式が不正: {version!r}")


@dataclass(frozen=True)
class Asset:
    platform: str
    source: str
    sha256: str


@dataclass(frozen=True)
class Manifest:
    version: str
    breaking: bool
    notes_url: str
    key_id: str
    generated_at: str
    assets: dict[str, Asset]


def build(
    *,
    version: str,
    packages: dict[str, Path],
    notes_url: str,
    breaking: bool,
    key_id: str,
    generated_at: str,
    signing_key: bytes,
) -> dict[str, Any]:
    """package の SHA-256 を含む manifest を作り、Ed25519 で署名する。"""
    validate_version(version)
    if not packages:
        raise ManifestError("package が 1 つもない")
    if not key_id:
        raise ManifestError("key_id が空")
    assets = {}
    for platform in sorted(packages):
        if not PLATFORM_PATTERN.match(platform):
            raise ManifestError(f"platform の形式が不正: {platform!r}")
        path = packages[platform]
        if not path.is_file():
            raise ManifestError(f"package が無い: {path}")
        assets[platform] = {
            "source": path.name,
            "sha256": sha256_file(path),
        }
    manifest: dict[str, Any] = {
        "schema": SCHEMA,
        "version": version,
        "breaking": breaking,
        "notes_url": notes_url,
        "generated_at": generated_at,
        "key_id": key_id,
        "assets": assets,
    }
    signature = ed25519.sign(signing_key, canonical_bytes(manifest))
    manifest["signature"] = base64.b64encode(signature).decode("ascii")
    return manifest


def dump(manifest: dict[str, Any]) -> bytes:
    """人が読める形で書き出す。署名は正規化 bytes に対して行うため整形は自由。"""
    return (
        json.dumps(manifest, sort_keys=True, indent=2, ensure_ascii=False) + "\n"
    ).encode("utf-8")


def parse(raw: bytes) -> tuple[Manifest, dict[str, Any]]:
    """形式だけを検証して読む。署名は verify_signature で確認する。"""
    try:
        manifest = json.loads(raw.decode("utf-8-sig"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ManifestError(f"manifest が JSON でない: {error}") from error
    if not isinstance(manifest, dict):
        raise ManifestError("manifest が object でない")
    unknown = set(manifest) - MANIFEST_FIELDS
    if unknown:
        raise ManifestError(f"未知の field: {', '.join(sorted(unknown))}")
    missing = MANIFEST_FIELDS - set(manifest)
    if missing:
        raise ManifestError(f"field が足りない: {', '.join(sorted(missing))}")
    if manifest["schema"] != SCHEMA:
        raise ManifestError(f"未対応の schema: {manifest['schema']!r}")
    if not isinstance(manifest["breaking"], bool):
        raise ManifestError("breaking が bool でない")
    if not isinstance(manifest["notes_url"], str) or not manifest["notes_url"]:
        raise ManifestError("notes_url が空")
    if not isinstance(manifest["key_id"], str) or not manifest["key_id"]:
        raise ManifestError("key_id が空")
    if not isinstance(manifest["generated_at"], str) or not manifest["generated_at"]:
        raise ManifestError("generated_at が空")
    if not isinstance(manifest["signature"], str):
        raise ManifestError("signature が文字列でない")
    validate_version(manifest["version"])
    raw_assets = manifest["assets"]
    if not isinstance(raw_assets, dict) or not raw_assets:
        raise ManifestError("assets が空")
    assets = {}
    for platform, asset in raw_assets.items():
        if not PLATFORM_PATTERN.match(platform):
            raise ManifestError(f"platform の形式が不正: {platform!r}")
        if not isinstance(asset, dict) or set(asset) != ASSET_FIELDS:
            raise ManifestError(f"{platform} の asset field が不一致")
        source = asset["source"]
        digest = asset["sha256"]
        if not isinstance(source, str) or not source:
            raise ManifestError(f"{platform} の source が空")
        validate_source(source)
        if not isinstance(digest, str) or not SHA256_PATTERN.match(digest):
            raise ManifestError(f"{platform} の sha256 が不正")
        assets[platform] = Asset(platform=platform, source=source, sha256=digest)
    return (
        Manifest(
            version=manifest["version"],
            breaking=manifest["breaking"],
            notes_url=manifest["notes_url"],
            key_id=manifest["key_id"],
            generated_at=manifest["generated_at"],
            assets=assets,
        ),
        manifest,
    )


def validate_source(source: str) -> None:
    """配布物の参照は相対パスか https URL だけを許す。"""
    if source.startswith("https://"):
        return
    if source.startswith("/") or "\\" in source or ":" in source:
        raise ManifestError(f"source が相対パスでない: {source!r}")
    parts = source.split("/")
    if any(part in ("", ".", "..") for part in parts):
        raise ManifestError(f"source に不正な要素がある: {source!r}")


def resolve_source(asset: Asset, base_dir: Path) -> Path:
    """相対 source を manifest と同じ置き場所から解決する。"""
    validate_source(asset.source)
    if asset.source.startswith("https://"):
        raise ManifestError("https の source は URL 取得経路で解決する")
    path = (base_dir / asset.source).resolve()
    if base_dir.resolve() != path.parent:
        raise ManifestError(f"source が manifest の隣にない: {asset.source!r}")
    return path


def verify_signature(manifest: dict[str, Any], trusted_keys: dict[str, str]) -> Manifest:
    parsed, _ = parse(canonical_bytes(manifest))
    encoded = trusted_keys.get(parsed.key_id)
    if encoded is None:
        raise ManifestError(f"信頼していない key_id: {parsed.key_id!r}")
    try:
        public = base64.b64decode(encoded, validate=True)
        signature = base64.b64decode(manifest["signature"], validate=True)
    except (ValueError, base64.binascii.Error) as error:
        raise ManifestError(f"署名の base64 が不正: {error}") from error
    if not ed25519.verify(public, canonical_bytes(signable_payload(manifest)), signature):
        raise ManifestError("manifest の署名が一致しない")
    return parsed


def verify_asset(
    manifest: Manifest, raw: dict[str, Any], platform: str, base_dir: Path
) -> Path:
    """署名済み manifest の SHA-256 と実 file を照合する。"""
    asset = manifest.assets.get(platform)
    if asset is None:
        raise ManifestError(f"platform が manifest にない: {platform!r}")
    path = resolve_source(asset, base_dir)
    if not path.is_file():
        raise ManifestError(f"package が無い: {path}")
    if sha256_file(path) != asset.sha256:
        raise ManifestError(f"package の SHA-256 が一致しない: {path.name}")
    return path
