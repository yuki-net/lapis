"""配布 package と Ed25519 署名付き manifest の生成・配置・検証。

経路は1本で、入り口だけを `--dest` / `--root` で切り替える。

    python scripts/package/distribution.py publish --binaries target/release --dest local:dist/v0.1.0
    python scripts/package/distribution.py publish --binaries target/release --dest github:v0.1.0
    python scripts/package/distribution.py verify --root url:https://example.invalid/lapis/stable

manifest の schema と install layout は docs/specs/distribution.md を正本とする。
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
import re
import subprocess
import sys
import tempfile
import urllib.error
import urllib.parse
import urllib.request
import zipfile
from pathlib import Path

SCHEMA_VERSION = 1
MANIFEST_NAME = "manifest.json"
# 検証用公開鍵の写し。信頼の根拠はリポジトリ/CI に固定した公開鍵で、このファイルは照合用。
PUBLIC_KEY_NAME = "manifest.pub"
DEFAULT_KEY_ID = "lapis-release-1"
DEFAULT_PLATFORM = "windows-x86_64"
# package に含める実行ファイル。lapis と lapis-host は必須。
BINARIES = ("lapis", "lapis-host", "lapis-web-bridge", "lapis-web")
REQUIRED_BINARIES = ("lapis", "lapis-host")
# zip を再現可能にするため、全エントリの更新時刻を固定する。
FIXED_ZIP_TIME = (1980, 1, 1, 0, 0, 0)


class DistributionError(Exception):
    """配布物の生成・配置・検証に失敗した。"""


# --- Ed25519 鍵 ---------------------------------------------------------


def load_private_key(path: Path):
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

    return Ed25519PrivateKey.from_private_bytes(read_hex_key(path, "秘密鍵"))


def load_public_key(value: str):
    """公開鍵は hex 文字列か、hex を書いたファイルのパスで渡す。"""
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey

    path = Path(value)
    if path.is_file():
        return Ed25519PublicKey.from_public_bytes(read_hex_key(path, "公開鍵"))
    try:
        key = bytes.fromhex(value.strip())
    except ValueError as error:
        raise DistributionError("公開鍵は hex 文字列か、その hex を書いたファイルです") from error
    if len(key) != 32:
        raise DistributionError("公開鍵の長さが 32 byte ではありません")
    return Ed25519PublicKey.from_public_bytes(key)


def read_hex_key(path: Path, label: str) -> bytes:
    try:
        raw = path.read_text(encoding="ascii").strip()
    except FileNotFoundError as error:
        raise DistributionError(f"{label}が見つかりません: {path}") from error
    try:
        key = bytes.fromhex(raw)
    except ValueError as error:
        raise DistributionError(f"{label}は hex 文字列ではありません: {path}") from error
    if len(key) != 32:
        raise DistributionError(f"{label}の長さが 32 byte ではありません: {path}")
    return key


def public_key_hex(private_key) -> str:
    from cryptography.hazmat.primitives import serialization

    return private_key.public_key().public_bytes(
        serialization.Encoding.Raw, serialization.PublicFormat.Raw
    ).hex()


def write_keypair(private_key_path: Path, public_key_path: Path) -> None:
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

    for path in (private_key_path, public_key_path):
        if path.exists():
            raise DistributionError(f"既存の鍵を上書きしません: {path}")
    private_key = Ed25519PrivateKey.generate()
    private_key_path.parent.mkdir(parents=True, exist_ok=True)
    public_key_path.parent.mkdir(parents=True, exist_ok=True)
    private_key_path.write_text(private_key.private_bytes_raw().hex() + "\n", encoding="ascii")
    try:
        os.chmod(private_key_path, 0o600)
    except OSError:
        pass
    public_key_path.write_text(public_key_hex(private_key) + "\n", encoding="ascii")


# --- manifest -----------------------------------------------------------


def canonical_bytes(payload: dict) -> bytes:
    """署名対象の正規化。key 順と区切りを固定し、signature は含めない。"""
    return json.dumps(payload, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")


def manifest_bytes(manifest: dict) -> bytes:
    return (json.dumps(manifest, indent=2, ensure_ascii=False, sort_keys=True) + "\n").encode("utf-8")


def sha256_hex(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


# version と platform はパスと URL の一部になるため、区切りや traversal を許さない。
SAFE_LABEL = re.compile(r"^[0-9A-Za-z][0-9A-Za-z._+-]*$")


def validate_label(value, label: str) -> None:
    if not isinstance(value, str) or not SAFE_LABEL.match(value):
        raise DistributionError(f"{label} に使えるのは英数字と . _ + - だけです: {value!r}")


def validate_payload(payload: dict) -> None:
    """旧 client が解釈できない形を生成・受理しない。"""
    expected = {"schema", "version", "breaking", "release_notes_url", "assets"}
    if set(payload) != expected:
        raise DistributionError(f"manifest の項目が schema と一致しません: {sorted(payload)}")
    if payload["schema"] != SCHEMA_VERSION:
        raise DistributionError(f"未対応の manifest schema です: {payload['schema']!r}")
    validate_label(payload["version"], "version")
    if not isinstance(payload["breaking"], bool):
        raise DistributionError("breaking は bool です")
    notes = payload["release_notes_url"]
    if not isinstance(notes, str) or urllib.parse.urlparse(notes).scheme not in ("http", "https"):
        raise DistributionError("release_notes_url は http(s) URL です")
    assets = payload["assets"]
    if not isinstance(assets, dict) or not assets:
        raise DistributionError("assets が空です")
    for platform, asset in assets.items():
        validate_label(platform, "platform")
        if not isinstance(asset, dict) or set(asset) != {"asset", "sha256", "size"}:
            raise DistributionError(f"asset の項目が schema と一致しません: {platform}")
        reference = asset["asset"]
        if not isinstance(reference, str) or not reference:
            raise DistributionError(f"asset の参照が不正です: {platform}")
        parsed = urllib.parse.urlparse(reference)
        if parsed.scheme not in ("", "http", "https"):
            raise DistributionError(f"asset の参照は URL か相対パスです: {platform}")
        if not parsed.scheme and (
            reference.startswith("//")
            or Path(reference).is_absolute()
            or ".." in Path(urllib.parse.unquote(reference)).parts
        ):
            raise DistributionError(f"asset の相対パスが配布元の外を指しています: {platform}")
        if not isinstance(asset["sha256"], str) or len(asset["sha256"]) != 64:
            raise DistributionError(f"asset の sha256 が不正です: {platform}")
        if not isinstance(asset["size"], int) or asset["size"] < 0:
            raise DistributionError(f"asset の size が不正です: {platform}")


def sign_manifest(payload: dict, private_key, key_id: str) -> dict:
    validate_payload(payload)
    return {
        **payload,
        "signature": {
            "algorithm": "ed25519",
            "key_id": key_id,
            "value": private_key.sign(canonical_bytes(payload)).hex(),
        },
    }


def verify_manifest(manifest: dict, public_key) -> dict:
    from cryptography.exceptions import InvalidSignature

    if not isinstance(manifest, dict):
        raise DistributionError("manifest が JSON object ではありません")
    signature = manifest.get("signature")
    if not isinstance(signature, dict) or set(signature) != {"algorithm", "key_id", "value"}:
        raise DistributionError("signature の項目が schema と一致しません")
    if signature["algorithm"] != "ed25519":
        raise DistributionError(f"未対応の署名方式です: {signature['algorithm']!r}")
    if not isinstance(signature["key_id"], str) or not signature["key_id"]:
        raise DistributionError("signature.key_id が不正です")
    try:
        signature_bytes = bytes.fromhex(signature["value"])
    except (TypeError, ValueError) as error:
        raise DistributionError("signature.value は hex 文字列です") from error
    payload = {key: value for key, value in manifest.items() if key != "signature"}
    validate_payload(payload)
    try:
        public_key.verify(signature_bytes, canonical_bytes(payload))
    except InvalidSignature as error:
        raise DistributionError("manifest の署名が一致しません") from error
    return payload


# --- package ------------------------------------------------------------


def collect_binaries(directory: Path) -> dict[str, bytes]:
    """ビルド出力から配布する実行ファイルだけを集める。locales と assets は exe に埋め込まれる。"""
    if not directory.is_dir():
        raise DistributionError(f"binary のディレクトリがありません: {directory}")
    files: dict[str, bytes] = {}
    for name in BINARIES:
        for candidate in (directory / f"{name}.exe", directory / name):
            if candidate.is_file():
                files[candidate.name] = candidate.read_bytes()
                break
    require_binaries(files)
    return files


def require_binaries(files: dict[str, bytes]) -> None:
    for name in REQUIRED_BINARIES:
        if not any(entry.startswith(f"{name}.") or entry == name for entry in files):
            raise DistributionError(f"必須の実行ファイルがありません: {name}")


def build_package(files: dict[str, bytes]) -> bytes:
    """package の中身を zip にする。展開先がそのまま versions/<version>/ になる。"""
    if not files:
        raise DistributionError("package 対象のファイルがありません")
    require_binaries(files)
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w", zipfile.ZIP_DEFLATED) as archive:
        for name in sorted(files):
            info = zipfile.ZipInfo(name, FIXED_ZIP_TIME)
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o644 << 16
            archive.writestr(info, files[name])
    return buffer.getvalue()


def asset_name(version: str, platform: str) -> str:
    return f"lapis-{version}-{platform}.zip"


# --- 配置先 -------------------------------------------------------------


class Destination:
    """配置先。生成物と検証は共通で、書き込み方だけが異なる。"""

    def asset_reference(self, name: str) -> str:
        raise NotImplementedError

    def write(self, files: dict[str, bytes]) -> None:
        raise NotImplementedError


def ordered(files: dict[str, bytes]) -> list[str]:
    # manifest は最後に置き、asset が揃う前に新しい版を指さないようにする。
    # 公開鍵の写しは最初に置く。
    def rank(name: str) -> tuple[int, str]:
        if name == PUBLIC_KEY_NAME:
            return (0, name)
        if name == MANIFEST_NAME:
            return (2, name)
        return (1, name)

    return sorted(files, key=rank)


class LocalDirectory(Destination):
    def __init__(self, directory: Path, asset_base: str | None = None):
        self.directory = directory
        self.asset_base = asset_base

    def asset_reference(self, name: str) -> str:
        if self.asset_base:
            return f"{self.asset_base.rstrip('/')}/{name}"
        # 相対パスは manifest の取得元を基準に解決する。
        return name

    def write(self, files: dict[str, bytes]) -> None:
        self.directory.mkdir(parents=True, exist_ok=True)
        for name in ordered(files):
            target = self.directory / name
            temporary = target.with_name(target.name + ".tmp")
            temporary.write_bytes(files[name])
            os.replace(temporary, target)


class GitHubRelease(Destination):
    """gh CLI で Release へ配置する。tag が無ければ作る。"""

    def __init__(self, tag: str, asset_base: str | None = None, target: str | None = None):
        self.tag = tag
        self.asset_base = asset_base
        self.target = target

    def asset_reference(self, name: str) -> str:
        base = self.asset_base or f"https://github.com/{repository()}/releases/download/{self.tag}"
        return f"{base.rstrip('/')}/{name}"

    def write(self, files: dict[str, bytes]) -> None:
        if not release_exists(self.tag):
            command = [
                "gh",
                "release",
                "create",
                self.tag,
                "--title",
                self.tag,
                "--notes",
                f"Lapis {self.tag}",
            ]
            # tag が既にあるなら gh に任せる。無い場合だけ現在の commit から作る。
            if not tag_exists(self.tag):
                command += ["--target", self.target or run(["git", "rev-parse", "HEAD"]).strip()]
            run(command)
        with tempfile.TemporaryDirectory(prefix="lapis-upload-") as directory:
            paths = []
            for name in ordered(files):
                path = Path(directory) / name
                path.write_bytes(files[name])
                paths.append(str(path))
            # asset を先に、manifest を最後に upload する。
            run(["gh", "release", "upload", self.tag, *paths, "--clobber"])


def tag_exists(tag: str) -> bool:
    result = subprocess.run(
        ["git", "rev-parse", "--verify", "--quiet", f"refs/tags/{tag}"], capture_output=True, text=True
    )
    return result.returncode == 0


def repository() -> str:
    return run(["gh", "repo", "view", "--json", "nameWithOwner", "-q", ".nameWithOwner"]).strip()


def release_exists(tag: str) -> bool:
    result = subprocess.run(["gh", "release", "view", tag], capture_output=True, text=True)
    return result.returncode == 0


def make_destination(specification: str, asset_base: str | None, target: str | None) -> Destination:
    kind, separator, value = specification.partition(":")
    if not separator or not value:
        raise DistributionError("--dest は local:<directory> か github:<tag> で指定します")
    if kind == "local":
        return LocalDirectory(Path(value), asset_base)
    if kind == "github":
        return GitHubRelease(value, asset_base, target)
    raise DistributionError(f"未対応の配置先です: {specification}")


# --- 読み取り root -------------------------------------------------------


class DistributionSource:
    """manifest と asset の取得元。ローカルフォルダと URL で同じ検証を使う。"""

    def read_manifest(self) -> dict:
        raise NotImplementedError

    def read(self, reference: str) -> bytes:
        raise NotImplementedError


class LocalSource(DistributionSource):
    def __init__(self, directory: Path):
        self.directory = directory

    def read_manifest(self) -> dict:
        return read_json(self.directory / MANIFEST_NAME)

    def read(self, reference: str) -> bytes:
        if urllib.parse.urlparse(reference).scheme in ("http", "https"):
            return read_url(reference)
        root = self.directory.resolve()
        path = (root / reference).resolve()
        if path != root and root not in path.parents:
            raise DistributionError(f"manifest の参照が配布フォルダの外を指しています: {reference}")
        try:
            return path.read_bytes()
        except FileNotFoundError as error:
            raise DistributionError(f"asset が見つかりません: {reference}") from error


class UrlSource(DistributionSource):
    def __init__(self, base: str):
        self.base = base.rstrip("/") + "/"

    def read_manifest(self) -> dict:
        return read_json_url(urllib.parse.urljoin(self.base, MANIFEST_NAME))

    def read(self, reference: str) -> bytes:
        if urllib.parse.urlparse(reference).scheme in ("http", "https"):
            return read_url(reference)
        url = urllib.parse.urljoin(self.base, reference)
        if not url.startswith(self.base):
            raise DistributionError(f"manifest の参照が配信元の外を指しています: {reference}")
        return read_url(url)


class ReleaseSource(DistributionSource):
    """GitHub Release から gh CLI で取得する。"""

    def __init__(self, tag: str):
        self.tag = tag
        self._directory = tempfile.TemporaryDirectory(prefix="lapis-release-")
        self._local = LocalSource(Path(self._directory.name))

    def materialize(self) -> None:
        run(["gh", "release", "download", self.tag, "--dir", self._directory.name, "--clobber"])

    def close(self) -> None:
        self._directory.cleanup()

    def read_manifest(self) -> dict:
        return self._local.read_manifest()

    def read(self, reference: str) -> bytes:
        return self._local.read(reference)


def make_source(specification: str) -> DistributionSource:
    kind, separator, value = specification.partition(":")
    if not separator or not value:
        raise DistributionError("--root は local:<directory>、url:<base>、release:<tag> で指定します")
    if kind == "local":
        return LocalSource(Path(value))
    if kind == "url":
        return UrlSource(value)
    if kind == "release":
        source = ReleaseSource(value)
        source.materialize()
        return source
    raise DistributionError(f"未対応の取得元です: {specification}")


def read_json(path: Path) -> dict:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as error:
        raise DistributionError(f"manifest が見つかりません: {path}") from error
    except json.JSONDecodeError as error:
        raise DistributionError(f"manifest が JSON ではありません: {path}: {error}") from error


def read_json_url(url: str) -> dict:
    try:
        return json.loads(read_url(url).decode("utf-8"))
    except json.JSONDecodeError as error:
        raise DistributionError(f"manifest が JSON ではありません: {url}: {error}") from error


def read_url(url: str) -> bytes:
    try:
        with urllib.request.urlopen(url, timeout=30) as response:
            return response.read()
    except (urllib.error.URLError, OSError) as error:
        raise DistributionError(f"取得できません: {url}: {error}") from error


def run(command: list[str]) -> str:
    result = subprocess.run(command, capture_output=True, text=True)
    if result.returncode != 0:
        raise DistributionError(
            f"コマンドが失敗しました: {' '.join(command)}: {result.stderr.strip() or result.stdout.strip()}"
        )
    return result.stdout


# --- 検証 ---------------------------------------------------------------


def verify(source: DistributionSource, public_key) -> dict:
    """manifest の署名と、指す asset の SHA-256・size を確認する。"""
    payload = verify_manifest(source.read_manifest(), public_key)
    for platform, asset in sorted(payload["assets"].items()):
        data = source.read(asset["asset"])
        if len(data) != asset["size"]:
            raise DistributionError(
                f"asset の size が一致しません: {platform}: manifest={asset['size']} actual={len(data)}"
            )
        if sha256_hex(data) != asset["sha256"]:
            raise DistributionError(f"asset の sha256 が一致しません: {platform}")
    return payload


# --- 生成と配置 ---------------------------------------------------------


def publish(
    *,
    version: str,
    packages: dict[str, dict[str, bytes]],
    destination: Destination,
    release_notes_url: str,
    breaking: bool,
    private_key,
    key_id: str,
    output: Path | None = None,
) -> dict:
    if not packages:
        raise DistributionError("platform が1つも指定されていません")
    written: dict[str, bytes] = {}
    assets: dict[str, dict[str, object]] = {}
    for platform, files in packages.items():
        name = asset_name(version, platform)
        package = build_package(files)
        written[name] = package
        assets[platform] = {
            "asset": destination.asset_reference(name),
            "sha256": sha256_hex(package),
            "size": len(package),
        }
    payload = {
        "schema": SCHEMA_VERSION,
        "version": version,
        "breaking": breaking,
        "release_notes_url": release_notes_url,
        "assets": assets,
    }
    manifest = sign_manifest(payload, private_key, key_id)
    written[MANIFEST_NAME] = manifest_bytes(manifest)
    written[PUBLIC_KEY_NAME] = (public_key_hex(private_key) + "\n").encode("ascii")
    destination.write(written)
    if output is not None:
        LocalDirectory(output).write(written)
    return manifest


def packages_from_arguments(
    binaries: list[str], package_dirs: list[str]
) -> dict[str, dict[str, bytes]]:
    if not binaries and not package_dirs:
        raise DistributionError("--binaries か --package-dir を1つ以上指定します")
    packages: dict[str, dict[str, bytes]] = {}
    for option, values in (("--binaries", binaries), ("--package-dir", package_dirs)):
        for value in values:
            platform, path = split_platform(value)
            if platform in packages:
                raise DistributionError(f"platform が重複しています: {platform}（{option}）")
            if option == "--binaries":
                packages[platform] = collect_binaries(Path(path))
                continue
            directory = Path(path)
            if not directory.is_dir():
                raise DistributionError(f"package のディレクトリがありません: {directory}")
            packages[platform] = {
                entry.relative_to(directory).as_posix(): entry.read_bytes()
                for entry in sorted(directory.rglob("*"))
                if entry.is_file()
            }
            require_binaries(packages[platform])
    return packages


def split_platform(value: str) -> tuple[str, str]:
    platform, separator, path = value.partition("=")
    if not separator:
        return DEFAULT_PLATFORM, value
    if not platform or not path:
        raise DistributionError(f"platform の指定が不正です: {value}")
    return platform, path


# --- CLI ----------------------------------------------------------------


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)

    keygen = commands.add_parser("keygen", help="署名鍵の組を作る。秘密鍵はリポジトリの外へ置く")
    keygen.add_argument("--private-key", type=Path, required=True)
    keygen.add_argument("--public-key", type=Path, required=True)

    publish_parser = commands.add_parser("publish", help="package と manifest を生成して配置する")
    publish_parser.add_argument("--version", required=True)
    publish_parser.add_argument(
        "--binaries", action="append", default=[], metavar="[PLATFORM=]DIR",
        help=f"lapis*.exe があるビルド出力。platform 省略時は {DEFAULT_PLATFORM}",
    )
    publish_parser.add_argument(
        "--package-dir", action="append", default=[], metavar="[PLATFORM=]DIR",
        help="package の中身を手で用意したディレクトリ",
    )
    publish_parser.add_argument("--dest", required=True, help="local:<directory> または github:<tag>")
    publish_parser.add_argument("--asset-base", help="asset の絶対 URL の基準。省略時は配置先に従う")
    publish_parser.add_argument("--github-target", help="Release を作る commit。省略時は HEAD")
    publish_parser.add_argument("--release-notes-url", required=True)
    publish_parser.add_argument("--breaking", action="store_true", help="データ世代が上がる破壊的変更")
    publish_parser.add_argument("--private-key", type=Path, required=True)
    publish_parser.add_argument("--key-id", default=DEFAULT_KEY_ID)
    publish_parser.add_argument("--output", type=Path, help="配置とは別に生成物の写しを残す")

    verify_parser = commands.add_parser("verify", help="manifest の署名と asset を検証する")
    verify_parser.add_argument("--root", required=True, help="local:<directory>、url:<base>、release:<tag>")
    verify_parser.add_argument("--public-key", required=True, help="hex 文字列、または hex を書いたファイル")

    arguments = parser.parse_args(argv)
    try:
        if arguments.command == "keygen":
            write_keypair(arguments.private_key, arguments.public_key)
            print(f"秘密鍵: {arguments.private_key}（リポジトリへ含めない）")
            print(f"公開鍵: {arguments.public_key}")
            return 0
        if arguments.command == "publish":
            manifest = publish(
                version=arguments.version,
                packages=packages_from_arguments(arguments.binaries, arguments.package_dir),
                destination=make_destination(arguments.dest, arguments.asset_base, arguments.github_target),
                release_notes_url=arguments.release_notes_url,
                breaking=arguments.breaking,
                private_key=load_private_key(arguments.private_key),
                key_id=arguments.key_id,
                output=arguments.output,
            )
            print(f"配置しました: {arguments.dest}")
            print(json.dumps(manifest, indent=2, ensure_ascii=False, sort_keys=True))
            return 0
        source = make_source(arguments.root)
        try:
            payload = verify(source, load_public_key(arguments.public_key))
        finally:
            if isinstance(source, ReleaseSource):
                source.close()
        print(
            f"検証しました: {payload['version']} "
            f"({', '.join(sorted(payload['assets']))}) breaking={payload['breaking']}"
        )
        return 0
    except DistributionError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
