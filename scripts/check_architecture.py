"""Cargo metadata に基づく内部依存の検査。"""
import json
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
OLD_DESKTOP_LAYOUT = {
    "TabGroup", "TabGroupId", "PanelPosition", "LayoutNode", "SplitDirection",
    "DropZone", "has_variable_splits", "group_for_tab", "leaf_group_ids",
    "focused_group", "source_group", "group_bounds", "open_group_tab",
    "panel_position", "panel_position_name", "toggle_panel", "request_panel_open",
    "ToggleBottomPanel", "toggle_bottom_panel",
    "select_panel_tab_in_group", "close_panel_tab_in_group", "transfer_tab_to_group",
}


OLD_ZONE_METHODS = {"open_panel", "close_panel", "request_panel_open_with_duration", "panel_contributions", "set_panel_active_view"}


def rust_identifiers(text):
    # 文字列・コメントは履歴用データを含み得るため、コード中の識別子のみ検査する。
    token = re.compile(r'//[^\n]*|/\*[\s\S]*?\*/|r(\#*)"[\s\S]*?"\1|"(?:\\.|[^"\\])*"|[A-Za-z_][A-Za-z_0-9]*')
    return [match.group() for match in token.finditer(text)
            if re.fullmatch(r'[A-Za-z_][A-Za-z_0-9]*', match.group())]


def strip_rust_comments(text):
    """Rustの行コメントとブロックコメントを空白に置き換えたテキストを返す。文字列リテラルは維持する。"""
    text = re.sub(r'/\*[\s\S]*?\*/', '', text)
    text = re.sub(r'//[^\n]*', '', text)
    return text


def check_desktop_layout(root=ROOT):
    errors = []
    source = root / "clients"
    for path in source.rglob("*.rs"):
        text = path.read_text(encoding="utf-8")
        identifiers = set(rust_identifiers(text))
        for name in sorted(identifiers & OLD_DESKTOP_LAYOUT):
            errors.append(f"{path.relative_to(root)}: 旧Desktop Layout識別子 {name}")
        code_text = strip_rust_comments(text)
        for name in OLD_ZONE_METHODS:
            if re.search(rf"\bfn\s+{name}\s*\([^)]*\bZonePosition\b", code_text, re.S):
                errors.append(f"{path.relative_to(root)}: Zone操作の旧Panel名 {name}")
        if re.search(r"\bstruct\s+PanelHost\s*\{[^}]*\bposition\s*:", code_text, re.S):
            errors.append(f"{path.relative_to(root)}: PanelHostへのZone所属の二重保持は禁止")
        if "/features/" in path.as_posix() and identifiers & {"PanelLayout", "ZonePolicy", "Arrangement"}:
            errors.append(f"{path.relative_to(root)}: FeatureからLayout内部表現へ依存")
        if "/features/" in path.as_posix() and re.search(
            r"\bviewport_size\s*\(|\buser_agent\b|navigator\.userAgent", code_text
        ):
            errors.append(
                f"{path.relative_to(root)}: Featureからviewport・user-agentへ依存（共通wrapperを使う）"
            )
        if "/features/" in path.as_posix() or path.name == "root.rs":
            if re.search(r"\bfile_dialog\s*\(\s*\)", code_text):
                errors.append(
                    f"{path.relative_to(root)}: Dialog を直接呼ぶ（DocumentClient の wrapper を使う）"
                )
        if "/features/" in path.as_posix():
            if re.search(r"\.overflow_(y_)?scroll\s*\(\s*\)", code_text):
                errors.append(
                    f"{path.relative_to(root)}: Featureがスクロールを直接組む（scroll_viewport を使う）"
                )
        if "/shell/main/" in path.as_posix() and identifiers & {"PanelLayout", "ZonePolicy", "Arrangement"}:
            errors.append(f"{path.relative_to(root)}: 描画・操作からLayout内部表現へ依存")
        if ("/shell/" in path.as_posix() and "/shell/main/header/" not in path.as_posix()
                or path.as_posix().endswith("/state/src/lib.rs")):
            for name in ("main-panel", "left-panel", "right-panel", "bottom-panel"):
                if re.search(rf'"{name}"', code_text):
                    errors.append(f"{path.relative_to(root)}: 旧Zone保存識別子 {name}")
    shell_mod = source / "ui/src/shell/mod.rs"
    if shell_mod.exists():
        module_text = shell_mod.read_text(encoding="utf-8")
        for module in ("panel_layout", "zone_policy", "zone"):
            if re.search(rf'\bpub\s*(?:\([^)]*\))?\s+mod\s+{module}\s*;', module_text):
                errors.append(f"{shell_mod.relative_to(root)}: {module} の公開は禁止")
    return errors
ALLOWED = {
    "models": {"models"},
    "protocol": {"protocol", "models"},
    "application": {"application", "models"},
    "runtime": {"runtime", "application", "models", "protocol"},
    "infrastructure": {"infrastructure", "application", "runtime", "models"},
    "transport": {"transport", "runtime", "protocol", "models", "client_core"},
    "host": {"host", "runtime", "application", "infrastructure", "transport", "models", "protocol"},
    "ui": {"ui", "desktop_client", "models", "protocol"},
    "desktop_client": {"desktop_client", "models", "protocol", "client_core"},
    "desktop_persistence": {"desktop_persistence", "desktop_client", "models"},
    "desktop_entry": {"desktop_entry", "ui", "desktop_client", "desktop_persistence", "host", "runtime", "application", "infrastructure", "transport", "models", "protocol"},
    "desktop_launcher": {"desktop_launcher"},
    "client": {"client", "models", "protocol", "client_core"},
    "client_core": {"client_core", "models", "protocol"},
}


def layer(manifest, root):
    relative = Path(manifest).resolve().relative_to(root.resolve()).as_posix()
    for prefix, value in [
        ("extensions/", "models"),
        ("contracts/domain/", "models"), ("contracts/transport/", "models"),
        ("contracts/protocol/", "protocol"), ("clients/transport/", "client_core"),
        ("host/application/", "application"), ("host/runtime/", "runtime"),
        ("host/workbench/", "runtime"),
        ("host/terminal/", "runtime"),
        ("host/documents/", "runtime"),
        ("host/adapters/", "infrastructure"), ("host/extensions/", "infrastructure"),
        ("host/workspaces/", "runtime"),
        ("host/auth/", "runtime"),
        ("host/tasks/", "runtime"),
        ("host/transport/", "transport"),
        ("host/app/", "host"), ("clients/ui/", "ui"),
        ("clients/state/", "desktop_client"),
        ("clients/desktop/persistence/", "desktop_persistence"),
        ("clients/desktop/launcher/", "desktop_launcher"),
        ("clients/desktop/app/", "desktop_entry"), ("clients/", "client"),
    ]:
        if relative.startswith(prefix):
            return value
    raise ValueError(f"未分類の workspace crate: {relative}")



# 同名の公開型を許す既知の組。新しい重複は設計の重複として拒否する。
DUPLICATE_TYPE_ALLOWLIST = {
    "OidcConfig": {"remote", "web-bridge"},
    "OidcError": {"remote", "web-bridge"},
    "TerminalSnapshot": {"protocol", "terminal"},
    "TerminalStatus": {"protocol", "terminal"},
}

PUBLIC_TYPE = re.compile(r"^pub (?:struct|enum|trait) ([A-Za-z_][A-Za-z0-9_]*)", re.M)


def crate_name_of(path, root):
    directory = path.parent
    while directory != root and not (directory / "Cargo.toml").exists():
        directory = directory.parent
    return directory.name


def check_duplicate_type_names(root=ROOT):
    """crate をまたいだ同名の公開型を検出する。"""
    source = root / "clients", root / "host", root / "contracts", root / "extensions"
    found = {}
    for base in source:
        if not base.exists():
            continue
        for path in base.rglob("*.rs"):
            posix = path.as_posix()
            if "/target/" in posix or "/vendor/" in posix or "/clients/mobile/" in posix:
                continue
            text = path.read_text(encoding="utf-8")
            crate = crate_name_of(path, root)
            for match in PUBLIC_TYPE.finditer(text):
                found.setdefault(match.group(1), set()).add(crate)

    errors = []
    for name, crates in sorted(found.items()):
        if len(crates) < 2:
            continue
        allowed = DUPLICATE_TYPE_ALLOWLIST.get(name)
        if allowed is not None and crates <= allowed:
            continue
        errors.append(
            f"公開型 {name} が複数 crate で定義されている: {', '.join(sorted(crates))}"
        )
    return errors


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
                if layers.get(name) in {"models", "protocol", "client_core", "application", "runtime", "infrastructure", "transport", "host", "desktop_client", "desktop_persistence"} and target in {"gpui", "rfd"}:
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
    errors = check(metadata, exceptions) + check_desktop_layout() + check_duplicate_type_names()
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print(f"Architecture OK ({len(metadata['workspace_members'])} crates, {len(exceptions)} explicit exceptions)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
