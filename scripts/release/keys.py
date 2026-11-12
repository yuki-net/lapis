"""署名鍵の生成と読み込み。

秘密鍵は repository に含めない。file もしくは環境変数で渡された path から読む。
検証用の公開鍵だけを `release/trusted-keys.json` に置く。
"""

from __future__ import annotations

import base64
import json
import os
import sys
from pathlib import Path

from . import ed25519
from .manifest import ManifestError

KEY_ENV = "LAPIS_RELEASE_SIGNING_KEY"
KEY_SCHEMA = 1
KEY_FIELDS = {"schema", "key_id", "algorithm", "seed"}
REGISTRY_FIELDS = {"schema", "keys"}


class SigningKeyError(ValueError):
    """署名鍵または公開鍵 registry が不正。"""


def generate(key_id: str, destination: Path) -> str:
    """seed を生成して destination に書き、公開鍵の base64 を返す。"""
    if not key_id:
        raise SigningKeyError("key_id が空")
    destination.parent.mkdir(parents=True, exist_ok=True)
    seed = os.urandom(32)
    payload = {
        "schema": KEY_SCHEMA,
        "key_id": key_id,
        "algorithm": "ed25519",
        "seed": base64.b64encode(seed).decode("ascii"),
    }
    body = (json.dumps(payload, indent=2, sort_keys=True) + "\n").encode("utf-8")
    # 作成時点で 0600 を適用し、既存 file は上書きしない。umask に左右されないよう
    # その後に chmod も行い、失敗は握りつぶさず呼び出し元へ返す。
    try:
        descriptor = os.open(
            destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600
        )
    except FileExistsError as error:
        raise SigningKeyError(f"鍵 file が既にある: {destination}") from error
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(body)
            stream.flush()
            os.fsync(stream.fileno())
        os.chmod(destination, 0o600)
    except BaseException:
        destination.unlink(missing_ok=True)
        raise
    return base64.b64encode(ed25519.public_key(seed)).decode("ascii")


def load_signing_key(explicit: str | None) -> tuple[str, bytes]:
    """(key_id, seed) を返す。CLI 引数に seed 自体を書かせない。"""
    raw_path = explicit or os.environ.get(KEY_ENV)
    if not raw_path:
        raise SigningKeyError(
            f"署名鍵が無い。--key <file> か環境変数 {KEY_ENV} で path を指定する"
        )
    path = Path(raw_path)
    if not path.is_file():
        raise SigningKeyError(f"署名鍵 file が無い: {path}")
    try:
        payload = json.loads(path.read_text(encoding="utf-8-sig"))
    except (OSError, json.JSONDecodeError) as error:
        raise SigningKeyError(f"署名鍵を読めない: {error}") from error
    if not isinstance(payload, dict):
        raise SigningKeyError("署名鍵が object でない")
    unknown = set(payload) - KEY_FIELDS
    if unknown:
        raise SigningKeyError(f"署名鍵に未知の field: {', '.join(sorted(unknown))}")
    if payload.get("schema") != KEY_SCHEMA:
        raise SigningKeyError(f"署名鍵の schema が不正: {payload.get('schema')!r}")
    if payload.get("algorithm") != "ed25519":
        raise SigningKeyError(
            f"署名鍵の algorithm が不正: {payload.get('algorithm')!r}"
        )
    key_id = payload.get("key_id")
    if not isinstance(key_id, str) or not key_id:
        raise SigningKeyError("署名鍵の key_id が空")
    try:
        seed = base64.b64decode(payload["seed"], validate=True)
    except (KeyError, ValueError, base64.binascii.Error) as error:
        raise SigningKeyError(f"署名鍵の seed が不正: {error}") from error
    if len(seed) != 32:
        raise SigningKeyError("署名鍵の seed は 32 byte")
    return key_id, seed


def load_trusted_keys(path: Path) -> dict[str, str]:
    """配布物に含める公開鍵 registry を読む。"""
    try:
        payload = json.loads(path.read_text(encoding="utf-8-sig"))
    except (OSError, json.JSONDecodeError) as error:
        raise ManifestError(f"公開鍵 registry を読めない: {error}") from error
    if not isinstance(payload, dict):
        raise ManifestError(f"公開鍵 registry が object でない: {path}")
    unknown = set(payload) - REGISTRY_FIELDS
    if unknown:
        raise ManifestError(
            f"公開鍵 registry に未知の field: {', '.join(sorted(unknown))}"
        )
    if payload.get("schema") != KEY_SCHEMA:
        raise ManifestError(f"公開鍵 registry の schema が不正: {path}")
    keys = payload.get("keys")
    if not isinstance(keys, dict):
        raise ManifestError(f"公開鍵 registry の keys が object でない: {path}")
    for key_id, encoded in keys.items():
        if not isinstance(key_id, str) or not key_id:
            raise ManifestError(f"公開鍵 registry の key_id が不正: {key_id!r}")
        try:
            raw = base64.b64decode(encoded, validate=True)
        except (ValueError, base64.binascii.Error) as error:
            raise ManifestError(f"{key_id} の公開鍵が base64 でない: {error}") from error
        if len(raw) != 32:
            raise ManifestError(f"{key_id} の公開鍵は 32 byte")
        if not ed25519.is_usable_public_key(raw):
            raise ManifestError(f"{key_id} の公開鍵が恒等元・小位数点")
    return dict(keys)


def warn_if_inside_repository(path: Path) -> None:
    """秘密鍵を repository 内へ置こうとした場合に警告する。"""
    repository = Path(__file__).resolve().parents[2]
    try:
        path.resolve().relative_to(repository)
    except ValueError:
        return
    print(
        f"警告: {path} は repository の中にある。秘密鍵は repository の外へ置く",
        file=sys.stderr,
    )
