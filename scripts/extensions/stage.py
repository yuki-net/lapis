"""拡張ディレクトリを発見し、配布用の実行コードとデータを組み立てる。"""

from __future__ import annotations

import argparse
import json
import platform
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import sys

from scripts.extensions.check import check


def stage(source: Path, output: Path, target: Path, *, release: bool = True) -> list[str]:
    source, output, target = source.resolve(), output.resolve(), target.resolve()
    if not source.is_dir() or (output.exists() and any(output.iterdir())):
        raise ValueError("拡張の入力がないか、出力先が空ではありません")
    output.mkdir(parents=True, exist_ok=True)
    for directory in sorted(source.iterdir()):
        if directory.is_symlink():
            raise ValueError("拡張ディレクトリに symlink は使えません")
        if not directory.is_dir():
            continue
        direct, component = directory / "extension.json", directory / "component" / "extension.json"
        if direct.is_symlink() or component.is_symlink() or (directory / "component").is_symlink():
            raise ValueError("拡張の定義に symlink は使えません")
        if direct.exists() and component.exists():
            raise ValueError(f"拡張の定義が複数あります: {directory}")
        manifest = direct if direct.exists() else component
        if not manifest.exists():
            continue
        if manifest.is_symlink() or manifest.stat().st_size > 1024 * 1024:
            raise ValueError("拡張の定義が不正です")
        definition = json.loads(manifest.read_text(encoding="utf-8"))
        files = definition.get("files")
        if not isinstance(files, list) or not files:
            raise ValueError("拡張の files が不正です")
        destination = output / directory.name
        destination.mkdir()
        component_cargo = manifest.parent / "Cargo.toml" if manifest == component else None
        wasm = None
        native = None
        native_config = directory / "native-build.json"
        if native_config.is_symlink():
            raise ValueError("native build 定義に symlink は使えません")
        use_native = (component_cargo is not None and native_config.exists()
                      and sys.platform == "win32" and platform.machine().lower() in ("amd64", "x86_64"))
        if use_native:
            if native_config.stat().st_size > 4096:
                raise ValueError("native build 定義が大きすぎます")
            config = json.loads(native_config.read_text(encoding="utf-8"))
            if not isinstance(config, dict) or set(config) != {"manifest"} or not isinstance(config["manifest"], str):
                raise ValueError("native build 定義が不正です")
            cargo_path = PurePosixPath(config["manifest"])
            if (not config["manifest"] or cargo_path.as_posix() != config["manifest"]
                    or cargo_path.is_absolute() or "\\" in config["manifest"] or ":" in config["manifest"]
                    or any(part in ("", ".", "..") for part in config["manifest"].split("/"))):
                raise ValueError("native manifest path が不正です")
            cargo_manifest = directory
            for part in cargo_path.parts:
                cargo_manifest = cargo_manifest / part
                if cargo_manifest.is_symlink():
                    raise ValueError("native manifest に symlink は使えません")
            if not cargo_manifest.is_file() or "extension.wasm" not in files:
                raise ValueError("native または component の定義がありません")
            requires = definition.get("requires")
            if (not isinstance(requires, list) or any(not isinstance(item, dict) for item in requires)
                    or any(item.get("contract") == "lapis.extension.native-abi" for item in requires)
                    or files.count("extension.wasm") != 1 or "native/windows-x86_64.dll" in files):
                raise ValueError("native の契約または file 定義が重複しています")
            command = ["cargo", "build", "--locked", "--manifest-path", str(cargo_manifest),
                       "--target-dir", str(target), "--message-format=json"]
            if release:
                command.append("--release")
            result = subprocess.run(command, check=True, text=True, stdout=subprocess.PIPE)
            artifacts = []
            for line in result.stdout.splitlines():
                message = json.loads(line)
                if (message.get("reason") == "compiler-artifact"
                        and Path(message.get("manifest_path", "")).resolve() == cargo_manifest.resolve()
                        and "cdylib" in message.get("target", {}).get("kind", [])):
                    artifacts.extend(Path(name) for name in message.get("filenames", []) if name.endswith(".dll"))
            if len(artifacts) != 1:
                raise ValueError("native library が一意ではありません")
            native = artifacts[0]
            files = ["native/windows-x86_64.dll" if name == "extension.wasm" else name for name in files]
            definition["files"] = files
            definition["requires"].append({"contract": "lapis.extension.native-abi", "minimum": "1.0.0", "maximum": "1.0.0"})
        (destination / "extension.json").write_text(json.dumps(definition, ensure_ascii=False), encoding="utf-8")
        if component_cargo is not None and not use_native:
            if "extension.wasm" not in files:
                raise ValueError("component の Wasm 定義がありません")
            command = ["cargo", "build", "--locked", "--manifest-path", str(component_cargo),
                       "--target", "wasm32-wasip2", "--target-dir", str(target), "--message-format=json"]
            if release:
                command.append("--release")
            result = subprocess.run(command, check=True, text=True, stdout=subprocess.PIPE)
            artifacts = []
            for line in result.stdout.splitlines():
                message = json.loads(line)
                if (message.get("reason") == "compiler-artifact"
                        and "cdylib" in message.get("target", {}).get("kind", [])):
                    artifacts.extend(Path(name) for name in message.get("filenames", []) if name.endswith(".wasm"))
            if len(artifacts) != 1:
                raise ValueError("component の Wasm が一意ではありません")
            wasm = artifacts[0]
        for name in files:
            if not isinstance(name, str):
                raise ValueError("拡張の file path が不正です")
            path = PurePosixPath(name)
            if (not name or path.as_posix() != name or path.is_absolute() or "\\" in name
                    or ":" in name or any(part in ("", ".", "..") for part in name.split("/"))):
                raise ValueError("拡張の file path が不正です")
            original = manifest.parent
            for part in path.parts:
                original = original / part
                if original.is_symlink():
                    raise ValueError("拡張の file に symlink は使えません")
            if name == "extension.wasm" and wasm is not None:
                original = wasm
            if name == "native/windows-x86_64.dll" and native is not None:
                original = native
            if not original.is_file():
                raise ValueError(f"拡張の file がありません: {name}")
            target_file = destination.joinpath(*path.parts)
            target_file.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(original, target_file)
    return check(output)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--target-dir", type=Path, default=Path("target/bundled-components"))
    parser.add_argument("--debug", action="store_true")
    args = parser.parse_args()
    print(json.dumps(stage(args.source, args.output, args.target_dir, release=not args.debug), ensure_ascii=False))


if __name__ == "__main__":
    main()
