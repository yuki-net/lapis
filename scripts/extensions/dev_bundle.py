"""開発セッション専用の署名済み同梱拡張を準備する。"""

from __future__ import annotations

import argparse
import base64
import json
import secrets
from pathlib import Path

from scripts.extensions.bundle import bundle
from scripts.extensions.stage import stage
from scripts.release.ed25519 import public_key


def prepare(source: Path, session: Path, target: Path) -> None:
    staged = session / "staged-extensions"
    packages = session / "bin" / "bundled-extensions"
    registry = session / "hosts" / "extensions" / "trusted-keys.json"
    if staged.exists() or packages.exists() or registry.exists():
        raise ValueError("開発セッションの拡張出力が既に存在します")
    stage(source, staged, target, release=False)
    seed = secrets.token_bytes(32)
    key_id = "lapis-dev-session"
    bundle(staged, packages, key_id=key_id, seed=seed)
    registry.parent.mkdir(parents=True, exist_ok=True)
    registry.write_text(json.dumps({"schema": 1, "keys": {
        key_id: base64.b64encode(public_key(seed)).decode("ascii"),
    }}), encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("session", type=Path)
    parser.add_argument("target", type=Path)
    args = parser.parse_args()
    prepare(args.source, args.session, args.target)


if __name__ == "__main__":
    main()
