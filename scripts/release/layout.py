"""インストール layout と pointer。

layout は後付けできないため、ここで固定する。launcher はこの API だけを使い、
version の探索や保持数を独自に決めない。

    <install root>/
      current.json           現在 version を指す pointer
      versions/<version>/    展開済み package と、その出所の manifest + zip
      staging/               download 済みで未適用の package
      data/<generation>/     DB / data。世代は軸B の release でだけ上げる
      credentials/           data 世代の外。失うと再登録が要る credential
"""

from __future__ import annotations

import contextlib
import json
import os
import shutil
import tempfile
import zipfile
from pathlib import Path

from . import manifest as manifest_module
from .publish import MANIFEST_NAME

POINTER_NAME = "current.json"
POINTER_SCHEMA = 1
LOCK_NAME = ".layout.lock"
VERSIONS_DIR = "versions"
STAGING_DIR = "staging"
DATA_DIR = "data"
CREDENTIALS_DIR = "credentials"
KEEP_VERSIONS = 2
RESERVED_ROOT_NAMES = {MANIFEST_NAME, POINTER_NAME}


class LayoutError(ValueError):
    """layout の破損、または package の展開に失敗。"""


def pointer_path(root: Path) -> Path:
    return root / POINTER_NAME


def versions_dir(root: Path) -> Path:
    return root / VERSIONS_DIR


def version_dir(root: Path, version: str) -> Path:
    _validate_name(version, "version")
    return versions_dir(root) / version


def staging_dir(root: Path) -> Path:
    return root / STAGING_DIR


def data_dir(root: Path, generation: str) -> Path:
    _validate_name(generation, "data generation")
    return root / DATA_DIR / generation


def credentials_dir(root: Path) -> Path:
    """data 世代の外に置く。失うと再登録が要る credential はここだけ。"""
    return root / CREDENTIALS_DIR


@contextlib.contextmanager
def _exclusive(root: Path):
    """install と activate を root 単位で排他する。

    `read_pointer` から版本の置換、pointer 更新、prune までの間に別の install が
    割り込むと、互いの前版を削除して pointer の参照先を失う。OS の file lock ではなく、
    同じ root を使う処理同士の相互排他だけを保証する簡易 lock とする。
    """
    root.mkdir(parents=True, exist_ok=True)
    path = root / LOCK_NAME
    handle = os.open(path, os.O_WRONLY | os.O_CREAT, 0o600)
    try:
        try:
            import fcntl
        except ImportError:  # Windows
            import msvcrt

            msvcrt.locking(handle, msvcrt.LK_LOCK, 1)
            try:
                yield
            finally:
                os.lseek(handle, 0, os.SEEK_SET)
                msvcrt.locking(handle, msvcrt.LK_UNLCK, 1)
        else:
            fcntl.flock(handle, fcntl.LOCK_EX)
            try:
                yield
            finally:
                fcntl.flock(handle, fcntl.LOCK_UN)
    finally:
        os.close(handle)


def _validate_name(value: str, label: str) -> None:
    if not value or "/" in value or "\\" in value or value in (".", ".."):
        raise LayoutError(f"{label} を directory 名に使えない: {value!r}")


def member_name(member: zipfile.ZipInfo) -> str:
    """ZIP member の名前を検証して返す。directory entry の末尾 `/` は除く。

    `zipfile` は Windows で `\\` を `/` に正規化して `member.filename` に入れるため、
    正規化前の `orig_filename` も併せて検査する。区切りや予約名の判定は展開と検証で
    同じ規則を使う。
    """
    for raw in (member.orig_filename, member.filename):
        _check_member_name(raw)
    name = member.filename[:-1] if member.filename.endswith("/") else member.filename
    if name.split("/")[0] in RESERVED_ROOT_NAMES:
        raise LayoutError(f"package が layout の予約名を使っている: {name!r}")
    return name


def _check_member_name(raw: str) -> None:
    name = raw[:-1] if raw.endswith("/") else raw
    parts = name.split("/")
    if (
        not name
        or name.startswith("/")
        or "\\" in raw
        or ":" in name
        or any(part in ("", ".", "..") for part in parts)
    ):
        raise LayoutError(f"package に不正な path がある: {raw!r}")


def read_pointer(root: Path) -> str | None:
    """現在 version を返す。pointer が無ければ None、壊れていれば例外。"""
    path = pointer_path(root)
    if not path.exists():
        return None
    try:
        payload = json.loads(path.read_text(encoding="utf-8-sig"))
    except (OSError, json.JSONDecodeError) as error:
        raise LayoutError(f"pointer を読めない: {error}") from error
    if not isinstance(payload, dict) or payload.get("schema") != POINTER_SCHEMA:
        raise LayoutError(f"pointer の schema が不正: {path}")
    if set(payload) != {"schema", "version"}:
        raise LayoutError(f"pointer の field が不正: {path}")
    version = payload["version"]
    if not isinstance(version, str) or not version:
        raise LayoutError(f"pointer の version が不正: {path}")
    return version


def write_pointer(root: Path, version: str) -> None:
    """pointer を原子的に置き換える。書き込み途中の値を見せない。"""
    directory = version_dir(root, version)
    if not directory.is_dir():
        raise LayoutError(f"version directory が無い: {directory}")
    root.mkdir(parents=True, exist_ok=True)
    payload = json.dumps(
        {"schema": POINTER_SCHEMA, "version": version}, sort_keys=True
    ) + "\n"
    handle, temporary = tempfile.mkstemp(dir=root, prefix=".current-", suffix=".tmp")
    try:
        with os.fdopen(handle, "w", encoding="utf-8", newline="\n") as stream:
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, pointer_path(root))
    except BaseException:
        Path(temporary).unlink(missing_ok=True)
        raise


def install(
    root: Path,
    distribution: Path,
    *,
    version: str,
    platform: str,
    trusted_keys: dict[str, str],
    replace: bool = False,
) -> Path:
    """配置済み配布物を検証し、versions/<version>/ へ展開して pointer を進める。

    distribution は manifest と package が同じ directory にある配置（ローカル複写でも
    Releases から download したものでもよい）。生成物と検証経路を分けない。

    既に同名 version がある場合、`replace=False` では失敗する。手動復旧で同じ version を
    入れ直す場合だけ `replace=True` を明示する。置き換えは検証と展開が終わってから行い、
    失敗時は元の directory を残す。

    検証は複写後の staged directory に対して行い、展開もその検証済み package から行う。
    元の distribution が検証後に差し替えられても、保存した package と展開内容が食い違わない。
    manifest が参照する platform の package はすべて保持する。`verify_distribution` は
    全 package を要求するため、選択した platform だけを残すと直後の検証が失敗する。

    pointer の読み取りから版本の置換、pointer 更新、prune までを root 単位で排他する。
    並行する install 同士が互いの前版を削除して pointer の参照先を失うのを防ぐ。
    """
    from . import verify as verify_module

    verified = verify_module.verify_distribution(distribution, trusted_keys)
    if verified.version != version:
        raise LayoutError(
            f"manifest の version ({verified.version}) が指定 ({version}) と違う"
        )
    if platform not in verified.packages:
        raise LayoutError(f"platform の package が無い: {platform!r}")
    with _exclusive(root):
        return _install_locked(
            root,
            distribution,
            verified,
            version=version,
            platform=platform,
            trusted_keys=trusted_keys,
            replace=replace,
        )


def _install_locked(
    root: Path,
    distribution: Path,
    verified,
    *,
    version: str,
    platform: str,
    trusted_keys: dict[str, str],
    replace: bool,
) -> Path:
    """`_exclusive` を保持した状態で version を配置する。"""
    from . import verify as verify_module

    target = version_dir(root, version)
    if target.exists() and not replace:
        raise LayoutError(
            f"version directory が既にある: {target}。入れ直すなら --replace を使う"
        )
    # pointer が壊れている場合は、展開する前に失敗させる。
    previous = read_pointer(root)
    versions_dir(root).mkdir(parents=True, exist_ok=True)
    staged = Path(tempfile.mkdtemp(dir=versions_dir(root), prefix=".extract-"))
    backup: Path | None = None
    try:
        shutil.copyfile(distribution / MANIFEST_NAME, staged / MANIFEST_NAME)
        for path in verified.packages.values():
            shutil.copyfile(path, staged / path.name)
        # 複写した staged を再検証する。以後の展開はこの検証済みコピーだけを使う。
        # 展開対象も再検証結果から選び直し、初回検証の manifest と食い違う package を
        # 展開しない。
        staged_verified = verify_module.verify_distribution(staged, trusted_keys)
        if staged_verified.version != version:
            raise LayoutError(
                f"複写した manifest の version ({staged_verified.version}) が違う"
            )
        staged_package = staged_verified.packages.get(platform)
        if staged_package is None:
            raise LayoutError(
                f"複写した manifest に platform の package が無い: {platform!r}"
            )
        _extract(staged_package, staged)
        skip = {MANIFEST_NAME} | {
            path.name for path in staged_verified.packages.values()
        }
        mismatched = verify_module.extracted_mismatches(
            staged, staged_package, skip=skip
        )
        if mismatched:
            raise LayoutError(
                "展開結果が package と一致しない: " + "; ".join(mismatched)
            )
        if target.exists():
            backup = Path(tempfile.mkdtemp(dir=versions_dir(root), prefix=".replace-"))
            backup.rmdir()
            os.replace(target, backup)
        try:
            os.replace(staged, target)
        except BaseException:
            if backup is not None:
                os.replace(backup, target)
                backup = None
            raise
    except BaseException:
        shutil.rmtree(staged, ignore_errors=True)
        raise
    if backup is not None:
        shutil.rmtree(backup, ignore_errors=True)
    write_pointer(root, version)
    _prune(root, keep=_retained_versions(root, version, previous))
    return target


def _retained_versions(root: Path, current: str, previous: str | None) -> list[str]:
    """残す version を KEEP_VERSIONS 件まで決める。

    同じ version を再導入した場合（previous == current）、pointer から前版を知ることは
    できない。その場合だけ導入済みの version を新しい順に補い、復旧先を消さない。
    """
    keep = [current]
    if previous and previous != current:
        keep.append(previous)
    elif previous == current:
        for name in _installed_versions_newest_first(root):
            if len(keep) >= KEEP_VERSIONS:
                break
            if name not in keep:
                keep.append(name)
    return keep


def _installed_versions_newest_first(root: Path) -> list[str]:
    directory = versions_dir(root)
    if not directory.is_dir():
        return []
    entries = [
        entry
        for entry in directory.iterdir()
        if entry.is_dir() and _installed_version(entry) == entry.name
    ]
    entries.sort(key=lambda entry: entry.stat().st_mtime, reverse=True)
    return [entry.name for entry in entries]


def activate(
    root: Path, version: str, *, platform: str, trusted_keys: dict[str, str]
) -> Path:
    """既に展開済みの version を検証して pointer だけを戻す。

    手動復旧で前版へ戻す経路。package の再取得や再展開をせず、壊れた version の
    directory には触らない。保存済み manifest と package に加えて、展開済み file も
    package の内容と照合する。実行 file だけが書き換わった version を復旧先にしない。

    install と同じ root 単位の排他に入る。復旧先の検証から pointer 更新までに別の
    install が割り込んで version を削除すると、存在しない version を指してしまう。
    """
    from . import verify as verify_module

    with _exclusive(root):
        target = version_dir(root, version)
        if not target.is_dir():
            raise LayoutError(f"version directory が無い: {target}")
        verified = verify_module.verify_install_version(
            target, trusted_keys, platform=platform
        )
        if verified.version != version:
            raise LayoutError(
                f"manifest の version ({verified.version}) が指定 ({version}) と違う"
            )
        write_pointer(root, version)
        return target


def _extract(package: Path, destination: Path) -> None:
    seen: set[str] = set()
    with zipfile.ZipFile(package) as archive:
        for member in archive.infolist():
            name = member_name(member)
            if name in seen:
                raise LayoutError(f"package に重複した path がある: {name!r}")
            seen.add(name)
            if member.is_dir():
                continue
            target = destination.joinpath(*name.split("/"))
            target.parent.mkdir(parents=True, exist_ok=True)
            with archive.open(member) as source, target.open("wb") as sink:
                shutil.copyfileobj(source, sink)


def _prune(root: Path, keep: list[str | None]) -> None:
    """keep にない version directory を削除する。

    削除するのは、directory 名と manifest の `version` が一致する「導入済み version」
    だけである。手動復旧で退避した `0.3.0.broken` のような directory は version が
    一致しないため残す。判断できない entry は削除しない。
    """
    retained = {name for name in keep if name}
    directory = versions_dir(root)
    if not directory.is_dir():
        return
    for entry in directory.iterdir():
        if not entry.is_dir() or entry.name in retained:
            continue
        if _installed_version(entry) != entry.name:
            continue
        shutil.rmtree(entry, ignore_errors=True)


def _installed_version(directory: Path) -> str | None:
    """directory が導入済み version なら manifest の version を返す。"""
    path = directory / MANIFEST_NAME
    if not path.is_file():
        return None
    try:
        parsed, _ = manifest_module.parse(path.read_bytes())
    except (ValueError, OSError):
        return None
    return parsed.version
