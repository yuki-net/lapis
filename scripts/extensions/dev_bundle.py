"""開発セッション専用の署名済み同梱拡張を準備する。"""

from __future__ import annotations

import argparse
import base64
from contextlib import contextmanager
import hashlib
import json
import os
import secrets
import tempfile
from pathlib import Path

from scripts.extensions.bundle import bundle
from scripts.extensions.stage import stage
from scripts.release.ed25519 import public_key


@contextmanager
def trust_lock(path: Path):
    with path.open("a+b") as file:
        file.seek(0)
        if os.name == "nt":
            import msvcrt
            msvcrt.locking(file.fileno(), msvcrt.LK_NBLCK, 1)
        else:
            import fcntl
            fcntl.flock(file.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        try:
            yield
        finally:
            if os.name == "nt":
                file.seek(0)
                msvcrt.locking(file.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                fcntl.flock(file.fileno(), fcntl.LOCK_UN)


def prepare(source: Path, session: Path, target: Path, *, hosts: Path | None = None,
            key_file: Path | None = None) -> None:
    staged = session / "staged-extensions"
    packages = session / "bin" / "bundled-extensions"
    registry = (hosts or session / "hosts") / "extensions" / "trusted-keys.json"
    if staged.exists() or packages.exists():
        raise ValueError("開発セッションの拡張出力が既に存在します")
    if hosts is not None and key_file is None:
        raise ValueError("永続 Host には開発用の鍵ファイルが必要です")
    stage(source, staged, target, release=False)
    if key_file is None:
        seed = secrets.token_bytes(32)
        key_id = "lapis-dev-session"
    else:
        key_file.parent.mkdir(parents=True, exist_ok=True)
        if key_file.is_symlink():
            raise ValueError("開発用鍵に symlink は使えません")
        try:
            fd = os.open(key_file, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        except FileExistsError:
            seed = key_file.read_bytes()
        else:
            seed = secrets.token_bytes(32)
            with os.fdopen(fd, "wb") as file:
                file.write(seed)
        if len(seed) != 32:
            raise ValueError("開発用鍵が不正です")
        key_id = "lapis-dev-" + hashlib.sha256(public_key(seed)).hexdigest()[:24]
    bundle(staged, packages, key_id=key_id, seed=seed)
    registry.parent.mkdir(parents=True, exist_ok=True)
    with trust_lock(registry.parent / ".dev-trust.lock"):
        if registry.is_symlink():
            raise ValueError("信頼済み鍵一覧に symlink は使えません")
        document = json.loads(registry.read_text(encoding="utf-8")) if registry.exists() else {
            "schema": 1, "keys": {},
        }
        if (not isinstance(document, dict) or set(document) != {"schema", "keys"}
                or document["schema"] != 1 or not isinstance(document["keys"], dict)):
            raise ValueError("信頼済み鍵一覧が不正です")
        encoded = base64.b64encode(public_key(seed)).decode("ascii")
        if key_id in document["keys"] and document["keys"][key_id] != encoded:
            raise ValueError("開発用鍵 ID が衝突しています")
        document["keys"][key_id] = encoded
        fd, temporary = tempfile.mkstemp(prefix=".trusted-keys-", dir=registry.parent)
        try:
            with os.fdopen(fd, "w", encoding="utf-8") as file:
                json.dump(document, file)
            os.replace(temporary, registry)
        finally:
            if os.path.exists(temporary):
                os.unlink(temporary)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("session", type=Path)
    parser.add_argument("target", type=Path)
    parser.add_argument("--hosts", type=Path)
    parser.add_argument("--key-file", type=Path)
    args = parser.parse_args()
    prepare(args.source, args.session, args.target, hosts=args.hosts, key_file=args.key_file)


if __name__ == "__main__":
    main()
