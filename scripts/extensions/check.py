"""同梱拡張の入力 manifest を発見し、package 作成を検証する。"""

from __future__ import annotations

import json
from pathlib import Path
import tempfile

from scripts.extensions.package import package


def check(root: Path) -> list[str]:
    if not root.is_dir():
        raise ValueError(f"拡張ディレクトリがありません: {root}")
    discovered = []
    ids = set()
    with tempfile.TemporaryDirectory(prefix="lapis-extensions-") as temporary:
        output = Path(temporary)
        for directory in sorted(root.iterdir()):
            if directory.is_symlink():
                raise ValueError(f"拡張ディレクトリに symlink は使えません: {directory}")
            manifest = directory / "extension.json"
            if not directory.is_dir():
                continue
            if manifest.is_symlink():
                raise ValueError(f"拡張の定義に symlink は使えません: {manifest}")
            if not manifest.exists():
                continue
            if manifest.stat().st_size > 1024 * 1024:
                raise ValueError(f"拡張の定義が不正または大きすぎます: {manifest}")
            data = json.loads(manifest.read_text(encoding="utf-8"))
            identifier = data.get("id") if isinstance(data, dict) else None
            if not isinstance(identifier, str) or not identifier:
                raise ValueError(f"拡張 ID がありません: {manifest}")
            if identifier in ids:
                raise ValueError(f"拡張 ID が重複しています: {identifier}")
            ids.add(identifier)
            package(directory, output / f"{len(discovered)}.lapis", seed=None, key_id=None)
            discovered.append(identifier)
    return discovered


def main() -> None:
    root = Path(__file__).resolve().parents[2] / "extensions"
    print(json.dumps(check(root), ensure_ascii=False))


if __name__ == "__main__":
    main()
