"""リリース鍵で同梱拡張を署名し、配布用ディレクトリに作る。"""

from __future__ import annotations

import argparse
from pathlib import Path

from scripts.extensions.check import check
from scripts.extensions.package import package
from scripts.release.keys import load_signing_key


def bundle(source: Path, output: Path, *, key_id: str, seed: bytes) -> None:
    # 先に全入力を検査し、重複や壊れた宣言を持つ配布物を作らない。
    check(source)
    if output.exists() and any(output.iterdir()):
        raise ValueError(f"同梱先が空ではありません: {output}")
    output.mkdir(parents=True, exist_ok=True)
    directories = [path for path in sorted(source.iterdir())
                   if path.is_dir() and (path / "extension.json").is_file()]
    for index, directory in enumerate(directories):
        package(directory, output / f"{index:04}.lapis", seed=seed, key_id=key_id)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--key")
    args = parser.parse_args()
    key_id, seed = load_signing_key(args.key)
    bundle(args.source, args.output, key_id=key_id, seed=seed)


if __name__ == "__main__":
    main()
