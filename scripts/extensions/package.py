"""拡張のファイルを `.lapis` にまとめる。秘密鍵はファイルからだけ読む。"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import re
from pathlib import Path, PurePosixPath
import tempfile
import zipfile

from scripts.release import ed25519
from scripts.release.manifest import canonical_bytes


def package(source: Path, output: Path, *, seed: bytes | None, key_id: str | None) -> None:
    source = source.resolve(strict=True)
    metadata_path = source / "extension.json"
    if metadata_path.is_symlink() or metadata_path.stat().st_size > 1024 * 1024:
        raise ValueError("extension.json が不正または大きすぎます")
    metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
    required = {"id", "version", "requires", "files"}
    if set(metadata) not in (required, required | {"capabilities"}):
        raise ValueError("extension.json の項目が不正です")
    if not isinstance(metadata["id"], str) or not isinstance(metadata["version"], str):
        raise ValueError("拡張 ID と version は文字列が必要です")
    if metadata["id"] == "lapis.core":
        raise ValueError("lapis.core は本体の翻訳先として予約されています")
    if not isinstance(metadata["requires"], list) or not metadata["requires"]:
        raise ValueError("requires は空でない配列が必要です")
    if (seed is None) != (key_id is None) or (key_id is not None and not key_id):
        raise ValueError("署名には秘密鍵と key ID の両方が必要です")
    if seed is not None and len(seed) != 32:
        raise ValueError("秘密鍵 seed は 32 byte 必要です")
    paths = metadata["files"]
    if not isinstance(paths, list) or not paths or len(paths) > 1023:
        raise ValueError("files は 1〜1023 個の path が必要です")
    entries = {}
    folded = {"manifest.json"}
    total = 0
    output_path = output.resolve()
    if output_path == metadata_path:
        raise ValueError("出力先に extension.json を指定できません")
    for name in paths:
        if not isinstance(name, str) or len(name.encode("utf-8")) > 255:
            raise ValueError("file path が不正です")
        path = PurePosixPath(name)
        if (not name or name != path.as_posix() or path.is_absolute() or "\\" in name
                or ":" in name or any(part in ("", ".", "..") or part.endswith((".", " ")) for part in name.split("/"))
                or name.lower() in folded):
            raise ValueError(f"file path が不正または重複しています: {name}")
        folded.add(name.lower())
        current = source
        for part in path.parts:
            current = current / part
            if current.is_symlink():
                raise ValueError(f"symlink は含められません: {name}")
        if current == output_path:
            raise ValueError(f"出力先に入力 file を指定できません: {name}")
        if not current.is_file() or current.stat().st_size > 32 * 1024 * 1024:
            raise ValueError(f"file が存在しないか大きすぎます: {name}")
        with current.open("rb") as handle:
            data = handle.read(32 * 1024 * 1024 + 1)
        if len(data) > 32 * 1024 * 1024:
            raise ValueError(f"file が大きすぎます: {name}")
        total += len(data)
        if total > 64 * 1024 * 1024:
            raise ValueError("package が大きすぎます")
        entries[name] = data
    manifest = {
        "schema": 1,
        "kind": "extension",
        "id": metadata["id"],
        "version": metadata["version"],
        "requires": metadata["requires"],
        "files": [{"path": name, "sha256": hashlib.sha256(data).hexdigest()} for name, data in sorted(entries.items())],
    }
    if "capabilities" in metadata:
        capabilities = metadata["capabilities"]
        if (not isinstance(capabilities, list) or len(capabilities) > 64
                or any(not isinstance(item, dict) or set(item) != {"id", "version"}
                       or not isinstance(item["id"], str) or not isinstance(item["version"], str)
                       for item in capabilities)):
            raise ValueError("capabilities が不正です")
        seen = set()
        for item in capabilities:
            identifier, version = item["id"], item["version"]
            if (len(identifier) > 64 or not re.fullmatch(r"[a-z0-9_.-]+", identifier)
                    or identifier.startswith(".") or identifier.endswith(".")
                    or not re.fullmatch(r"[1-9][0-9]*\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", version)
                    or any(int(part) > 2**64 - 1 for part in version.split("."))
                    or (identifier, version) in seen):
                raise ValueError("capability の ID または version が不正です")
            seen.add((identifier, version))
        manifest["schema"] = 2
        manifest["capabilities"] = capabilities
    if seed is not None:
        manifest["key_id"] = key_id
        manifest["signature"] = base64.b64encode(ed25519.sign(seed, canonical_bytes(manifest))).decode("ascii")
    manifest_bytes = canonical_bytes(manifest)
    if len(manifest_bytes) > 32 * 1024 * 1024 or total + len(manifest_bytes) > 64 * 1024 * 1024:
        raise ValueError("manifest を含む package が大きすぎます")
    output.parent.mkdir(parents=True, exist_ok=True)
    fd, name = tempfile.mkstemp(prefix=".lapis-", dir=output.parent)
    try:
        with os.fdopen(fd, "w+b") as file:
            with zipfile.ZipFile(file, "w", compression=zipfile.ZIP_DEFLATED) as archive:
                def write(name: str, data: bytes) -> None:
                    info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
                    info.create_system = 3
                    info.external_attr = 0o100644 << 16
                    info.compress_type = zipfile.ZIP_DEFLATED
                    archive.writestr(info, data)

                write("manifest.json", manifest_bytes)
                for path, data in sorted(entries.items()):
                    write(path, data)
        if os.path.getsize(name) > 64 * 1024 * 1024:
            raise ValueError("圧縮後の package が大きすぎます")
        os.replace(name, output)
    finally:
        if os.path.exists(name):
            os.unlink(name)


def main() -> None:
    parser = argparse.ArgumentParser(description="拡張 package を作成する")
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    auth = parser.add_mutually_exclusive_group(required=True)
    auth.add_argument("--development", action="store_true")
    auth.add_argument("--seed-file", type=Path)
    parser.add_argument("--key-id")
    args = parser.parse_args()
    if args.development and args.key_id:
        parser.error("--development に --key-id は指定できません")
    if args.seed_file and not args.key_id:
        parser.error("--seed-file には --key-id が必要です")
    package(args.source, args.output, seed=args.seed_file.read_bytes() if args.seed_file else None, key_id=args.key_id)


if __name__ == "__main__":
    main()
