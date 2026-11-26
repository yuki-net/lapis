"""Cargo metadata に基づく内部依存の検査。"""
import json
import os
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]

# 行数上限の検査で走査しないディレクトリ（生成物・外部コード）。
FILE_LENGTH_SKIP = {"target", "vendor", "node_modules", ".git"}
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
    client_root = root / "clients"
    # Desktop の entry は apps/ 側にもあるため、Layout の旧識別子は両方から検査する。
    sources = [client_root, root / "apps", root / "workbench", root / "tools"]
    for path in (path for source in sources if source.exists() for path in source.rglob("*.rs")):
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
        if ("/shell/" in path.as_posix() and "/main/header/" not in path.as_posix()
                or path.as_posix().endswith("/state/src/lib.rs")):
            for name in ("main-panel", "left-panel", "right-panel", "bottom-panel"):
                if re.search(rf'"{name}"', code_text):
                    errors.append(f"{path.relative_to(root)}: 旧Zone保存識別子 {name}")
    shell_mod = root / "workbench/shell/src/lib.rs"
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
    "ui": {"ui", "workbench_shell", "workbench_ui", "desktop_client", "models", "protocol"},
    "desktop_client": {"desktop_client", "models", "protocol", "client_core"},
    "desktop_persistence": {"desktop_persistence", "desktop_client", "models"},
    "desktop_entry": {"desktop_entry", "ui", "desktop_client", "desktop_persistence", "desktop_launcher", "host", "runtime", "application", "infrastructure", "transport", "models", "protocol"},
    "desktop_launcher": {"desktop_launcher"},
    "web_entry": {"web_entry", "ui", "desktop_client", "models", "protocol", "client_core"},
    "client": {"client", "models", "protocol", "client_core"},
    "client_core": {"client_core", "models", "protocol"},
    "workbench_ui": {"workbench_ui", "models", "protocol"},
    "workbench_shell": {"workbench_shell", "workbench_ui", "desktop_client", "models", "protocol"},
    "tools": {"tools", "desktop_client", "models", "protocol"},
}


def layer(manifest, root):
    relative = Path(manifest).resolve().relative_to(root.resolve()).as_posix()
    # 拡張は役割ディレクトリで構成する（`contract` / `logic` / `host` / `adapter` / `renderer`）。
    # Host 状態サービスは runtime、OS に触れる実装は infrastructure として扱い、
    # 拡張の中にあっても層の意味を変えない。
    parts = relative.split("/")
    if len(parts) >= 3 and parts[0] == "extensions":
        roles = {"contract": "models", "renderer": "models", "logic": "runtime", "host": "runtime", "adapter": "infrastructure", "native": "infrastructure"}
        if parts[2] in roles:
            return roles[parts[2]]
    for prefix, value in [
        ("extensions/", "models"),
        ("protocol/extension-api/", "models"),
        ("protocol/types/", "models"), ("protocol/wire/dto/", "models"),
        ("protocol/wire/client-api/", "protocol"), ("protocol/wire-client/", "client_core"),
        ("host/application/", "application"), ("host/runtime/", "runtime"),
        ("host/domains/", "runtime"),
        ("host/adapters/", "infrastructure"), ("host/extensions/", "infrastructure"),
        ("host/extensions-entry/", "infrastructure"),
        ("host/transport/", "transport"),
        ("host/app/", "host"), ("workbench/ui/", "ui"),
        ("workbench/ui-kit/", "workbench_ui"),
        ("workbench/shell/", "workbench_shell"),
        ("tools/", "tools"),
        ("apps/web/", "web_entry"),
        ("workbench/state/", "desktop_client"),
        ("apps/desktop/persistence/", "desktop_persistence"),
        ("apps/launcher/", "desktop_launcher"),
        ("apps/desktop/app/", "desktop_entry"),
        ("apps/", "client"), ("clients/", "client"),
    ]:
        if relative.startswith(prefix):
            return value
    raise ValueError(f"未分類の workspace crate: {relative}")



# 同名の公開型を許す既知の組。新しい重複は設計の重複として拒否する。
DUPLICATE_TYPE_ALLOWLIST = {
}

PUBLIC_TYPE = re.compile(r"^pub (?:struct|enum|trait) ([A-Za-z_][A-Za-z0-9_]*)", re.M)


def crate_name_of(path, root):
    """crate の同一性は package 名で判定する。

    `extensions/*/contract` のようにディレクトリ名が重複するため、ディレクトリ名では
    別 crate の型を同じ crate のものとして扱ってしまう。
    """
    directory = path.parent
    while directory != root and not (directory / "Cargo.toml").exists():
        directory = directory.parent
    manifest = directory / "Cargo.toml"
    if manifest.exists():
        match = re.search(r'^name\s*=\s*"([^"]+)"', manifest.read_text(encoding="utf-8"), re.M)
        if match:
            return match.group(1)
    return directory.name


def check_duplicate_type_names(root=ROOT):
    """crate をまたいだ同名の公開型を検出する。"""
    source = root / "apps", root / "clients", root / "host", root / "protocol", root / "extensions", root / "workbench", root / "tools"
    found = {}
    for base in source:
        if not base.exists():
            continue
        for path in base.rglob("*.rs"):
            posix = path.as_posix()
            if "/target/" in posix or "/vendor/" in posix or "/apps/mobile/" in posix:
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
    seen = set()
    pending = ["host"] if "host" in packages else []
    while pending:
        name = pending.pop()
        if name in seen:
            continue
        seen.add(name)
        for dependency in packages[name]["dependencies"]:
            target = dependency["name"]
            if dependency.get("kind") == "dev" or target not in packages:
                continue
            manifest = Path(packages[target]["manifest_path"]).as_posix()
            if "/extensions/terminal/" in manifest:
                errors.append(f"host -> {target}: terminal extension dependency")
            else:
                pending.append(target)
    for edge in set(exceptions) - used:
        errors.append(f"不要または無効な例外: {edge}")
    return errors


def read_file_length_baseline(path=None):
    """`lints/baseline.json` から上限と既存違反を読む。"""
    path = Path(path) if path is not None else ROOT / "lints/baseline.json"
    data = json.loads(path.read_text(encoding="utf-8"))
    limit = data.get("limit")
    files = data.get("files")
    if not isinstance(limit, int) or limit <= 0:
        raise ValueError(f"{path}: limit は正の整数で指定する")
    if not isinstance(files, dict) or not all(
        isinstance(name, str) and isinstance(lines, int) for name, lines in files.items()
    ):
        raise ValueError(f"{path}: files は path -> 行数の object で指定する")
    return limit, files


def file_length_errors(sizes, baseline, limit):
    """行数の上限と baseline のラチェットを判定する。

    - 上限超過の新規ファイルはエラー（baseline への記録は人間の判断）
    - baseline より増えたらエラー
    - baseline より減ったら、baseline を下げるまでエラー（ラチェット）
    - 上限以下になったファイルは baseline から削除する
    """
    errors = []
    for name, lines in sorted(sizes.items()):
        recorded = baseline.get(name)
        if lines > limit:
            if recorded is None:
                errors.append(
                    f"{name}: {lines} 行は上限 {limit} 行を超える"
                    "（分割するか、人間が lints/baseline.json に記録する）"
                )
            elif lines > recorded:
                errors.append(f"{name}: {recorded} 行 -> {lines} 行に増えた（上限 {limit} 行）")
            elif lines < recorded:
                errors.append(
                    f"{name}: {recorded} 行 -> {lines} 行に減った。"
                    f"lints/baseline.json を {lines} に下げる"
                )
        elif recorded is not None:
            errors.append(
                f"{name}: 上限 {limit} 行以下になったので lints/baseline.json から削除する"
            )
    for name in sorted(set(baseline) - set(sizes)):
        errors.append(f"{name}: lints/baseline.json にあるがファイルが無い")
    return errors


def rust_line_counts(root=ROOT):
    """リポジトリ内の Rust ファイルの行数（生成物・vendor を除く）。"""
    sizes = {}
    for directory, dirnames, filenames in os.walk(root):
        dirnames[:] = [name for name in dirnames if name not in FILE_LENGTH_SKIP]
        for name in filenames:
            if not name.endswith(".rs"):
                continue
            path = Path(directory) / name
            text = path.read_text(encoding="utf-8")
            sizes[path.relative_to(root).as_posix()] = len(text.splitlines())
    return sizes


def check_file_length(root=ROOT, baseline_path=None):
    """`lints/baseline.json` を基準に、Rust ファイルの行数を検査する。"""
    limit, baseline = read_file_length_baseline(
        baseline_path if baseline_path is not None else Path(root) / "lints/baseline.json"
    )
    return file_length_errors(rust_line_counts(root), baseline, limit)


def main():
    metadata = json.loads(subprocess.check_output([
        "cargo", "metadata", "--locked", "--no-deps", "--format-version", "1",
    ], cwd=ROOT))
    exceptions = json.loads((ROOT / "scripts/architecture-exceptions.json").read_text(encoding="utf-8"))
    errors = (
        check(metadata, exceptions)
        + check_desktop_layout()
        + check_duplicate_type_names()
        + check_file_length()
    )
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print(f"Architecture OK ({len(metadata['workspace_members'])} crates, {len(exceptions)} explicit exceptions)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
