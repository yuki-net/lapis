"""配布物の配置。

経路は 1 本だけにする。生成した artifact directory を、そのままローカルへ複写するか
GitHub Releases の asset として upload する。どちらの入り口も manifest と package の
形式、検証方法を変えない。

信頼の起点は repository に置く `release/trusted-keys.json` だけであり、配布物に
含めた公開鍵を信頼の起点にしない。

GitHub Releases への書き込みは既定で dry-run とし、実行には `execute=True` と
remote に既に存在する tag を要求する。tag と Release の新規作成はこの module の
責務外であり、`gh release create` は使わない。既存 Release への asset 差し替えだけ行う。
"""

from __future__ import annotations

import shutil
import subprocess
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Protocol

from . import manifest as manifest_module

MANIFEST_NAME = "manifest.json"


class PublishError(ValueError):
    """配置先の指定、または GitHub Releases の前提が不正。"""


Runner = Callable[[list[str]], None]


class Remote(Protocol):
    """GitHub 側の状態参照。テストでは network を触らない実装に差し替える。"""

    def release_exists(self, tag: str) -> bool: ...

    def tag_exists(self, tag: str) -> bool: ...


@dataclass(frozen=True)
class GhRemote:
    repo: str

    def _git_url(self) -> str:
        """`git ls-remote` に渡す URL。`owner/repo` の slug は Git が解決できない。"""
        if "://" in self.repo or self.repo.startswith("git@"):
            return self.repo
        return f"https://github.com/{self.repo}"

    def release_exists(self, tag: str) -> bool:
        result = subprocess.run(
            ["gh", "release", "view", tag, "--repo", self.repo, "--json", "tagName"],
            check=False,
            capture_output=True,
            text=True,
        )
        return result.returncode == 0

    def tag_exists(self, tag: str) -> bool:
        result = subprocess.run(
            [
                "git",
                "ls-remote",
                "--tags",
                "--exit-code",
                self._git_url(),
                f"refs/tags/{tag}",
            ],
            check=False,
            capture_output=True,
            text=True,
        )
        return result.returncode == 0 and bool(result.stdout.strip())


def _run(command: list[str]) -> None:
    subprocess.run(command, check=True)


@dataclass(frozen=True)
class Artifacts:
    """配置する 1 組。署名済み manifest と、それが参照する package。"""

    directory: Path
    trusted_keys: dict[str, str]

    def verified(self):
        """署名と SHA-256 を検証し、(manifest, payload) を返す。"""
        manifest_path = self.directory / MANIFEST_NAME
        if not manifest_path.is_file():
            raise PublishError(f"manifest が無い: {manifest_path}")
        raw = manifest_path.read_bytes()
        parsed, payload = manifest_module.parse(raw)
        manifest_module.verify_signature(payload, self.trusted_keys)
        return parsed, payload

    def verified_files(self):
        """署名と SHA-256 を検証し、(manifest, 配置する file の一覧) を返す。"""
        manifest_path = self.directory / MANIFEST_NAME
        parsed, payload = self.verified()
        files = [manifest_path]
        for asset in parsed.assets.values():
            files.append(
                manifest_module.verify_asset(
                    parsed, payload, asset.platform, self.directory
                )
            )
        return parsed, files

    def files(self) -> list[Path]:
        """署名と SHA-256 を検証してから、配置する file を返す。"""
        _, files = self.verified_files()
        return files


def publish_local(artifacts: Artifacts, destination: Path) -> Path:
    """ローカル folder / 静的配置へ同じ形式で複写する。"""
    files = artifacts.files()
    if destination.exists() and destination.resolve() == artifacts.directory.resolve():
        raise PublishError("配置先が生成元と同じ")
    destination.mkdir(parents=True, exist_ok=True)
    for path in files:
        shutil.copyfile(path, destination / path.name)
    return destination


def upload_command(tag: str, repo: str, files: list[Path]) -> list[str]:
    return [
        "gh",
        "release",
        "upload",
        tag,
        "--repo",
        repo,
        "--clobber",
        *[str(path) for path in files],
    ]


def create_command(
    tag: str, repo: str, files: list[Path], title: str, notes_file: Path
) -> list[str]:
    """既存 tag に対する Release を 1 つ作る。tag 自体は作らない。"""
    return [
        "gh",
        "release",
        "create",
        tag,
        "--repo",
        repo,
        "--title",
        title,
        "--notes-file",
        str(notes_file),
        "--verify-tag",
        *[str(path) for path in files],
    ]


def publish_github_releases(
    artifacts: Artifacts,
    *,
    tag: str,
    repo: str,
    remote: Remote,
    execute: bool = False,
    allow_create: bool = False,
    title: str = "",
    notes_file: Path | None = None,
    runner: Runner = _run,
) -> list[str]:
    """GitHub Releases へ配置する。

    `execute=False` では remote 参照も upload も行わず、実行予定の command を返す。
    ここで GitHub を読みに行かないため、テストや手元確認が Release 状態に依存しない。

    `execute=True` では tag の存在を必須にする。Release が無い場合、`allow_create`
    が False なら失敗する。tag 駆動の CI だけが `allow_create=True` を渡し、既存 tag に
    対して `gh release create --verify-tag` を 1 回だけ実行する。tag 自体はどちらの
    経路でも作成しない。tag は manifest の version と一致することを dry-run でも
    確認する。別 version の成果物を既存 Release へ上書きしないためである。
    """
    if not tag.startswith("v") or len(tag) < 2 or ".." in tag:
        raise PublishError(f"tag は v で始まる version のみ: {tag!r}")
    parsed, files = artifacts.verified_files()
    # 公開先と配布物の対応を remote 操作の前に確認する。`--clobber` で別 version の
    # 成果物を既存 tag の Release へ上書きしないため。
    if tag != f"v{parsed.version}":
        raise PublishError(
            f"tag ({tag}) と manifest の version ({parsed.version}) が一致しない"
        )
    if not execute:
        # dry-run では remote を読まないため、既存 Release があるか分からない。
        # どのみち実行しないので、作成を伴わない upload の形だけを示す。
        return upload_command(tag, repo, files)
    if not remote.tag_exists(tag):
        raise PublishError(f"remote に tag が無い: {tag}。tag の作成はこの tool の責務外")
    if remote.release_exists(tag):
        command = upload_command(tag, repo, files)
    elif allow_create:
        if notes_file is None or not notes_file.is_file():
            raise PublishError("Release の作成には notes_file が必要")
        command = create_command(tag, repo, files, title or f"Lapis {tag}", notes_file)
    else:
        raise PublishError(
            f"remote に Release が無い: {tag}。作成は --create を明示した場合だけ行う"
        )
    runner(command)
    return command

