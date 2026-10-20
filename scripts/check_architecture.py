"""Cargo metadata に基づく内部依存の検査。"""
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
ALLOWED = {
    "models": {"models"},
    "protocol": {"protocol", "models"},
    "application": {"application", "models"},
    "runtime": {"runtime", "application", "models", "protocol"},
    "infrastructure": {"infrastructure", "application", "runtime", "models"},
    "transport": {"transport", "runtime", "protocol", "models"},
    "host": {"host", "runtime", "application", "infrastructure", "transport", "models", "protocol"},
    "ui": {"ui", "application", "models", "protocol"},
    "desktop_entry": {"desktop_entry", "ui", "runtime", "application", "infrastructure", "transport", "models", "protocol"},
    "client": {"client", "models", "protocol"},
}


def layer(manifest, root):
    relative = Path(manifest).resolve().relative_to(root.resolve()).as_posix()
    for prefix, value in [
        ("shared/models/", "models"), ("shared/protocol/", "protocol"),
        ("backend/application/", "application"), ("backend/runtime/", "runtime"),
        ("backend/infrastructure/", "infrastructure"), ("backend/transport/", "transport"),
        ("backend/host/", "host"), ("apps/desktop/ui/", "ui"),
        ("apps/desktop/app/", "desktop_entry"), ("apps/", "client"),
    ]:
        if relative.startswith(prefix):
            return value
    raise ValueError(f"未分類の workspace crate: {relative}")


def check(metadata, exceptions, root=ROOT):
    members = set(metadata["workspace_members"])
    packages = {item["name"]: item for item in metadata["packages"] if item["id"] in members}
    errors = []
    layers = {}
    used = set()
    for name, package in packages.items():
        try:
            layers[name] = layer(package["manifest_path"], root)
        except ValueError as error:
            errors.append(str(error))
    for name, package in packages.items():
        for dependency in package["dependencies"]:
            target = dependency["name"]
            if target not in packages:
                if layers.get(name) in {"models", "protocol", "application", "runtime", "infrastructure", "transport", "host"} and target in {"gpui", "rfd"}:
                    errors.append(f"{name} -> {target}: Client 固有依存")
                continue
            if name not in layers or target not in layers:
                continue
            edge = f"{name} -> {target}"
            if layers[target] not in ALLOWED[layers[name]]:
                if edge in exceptions and exceptions[edge].strip():
                    used.add(edge)
                else:
                    errors.append(f"{edge}: {layers[name]} から {layers[target]} への禁止依存")
    for edge in set(exceptions) - used:
        errors.append(f"不要または無効な例外: {edge}")
    return errors


def main():
    metadata = json.loads(subprocess.check_output([
        "cargo", "metadata", "--locked", "--no-deps", "--format-version", "1",
    ], cwd=ROOT))
    exceptions = json.loads((ROOT / "scripts/architecture-exceptions.json").read_text(encoding="utf-8"))
    errors = check(metadata, exceptions)
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print(f"Architecture OK ({len(metadata['workspace_members'])} crates, {len(exceptions)} explicit exceptions)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
