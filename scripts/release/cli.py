"""配布物 CLI。

    python -m scripts.release keygen --key-id lapis-release --out <file>
    python -m scripts.release build --version 0.2.0 \
        --platform windows-x86_64=<dir> --platform linux-x86_64=<dir> \
        --out dist --notes-url https://example.invalid/releases/v0.2.0
    python -m scripts.release publish --source dist --local <dir>
    python -m scripts.release publish --source dist --github --tag v0.2.0
    python -m scripts.release verify --source dist
    python -m scripts.release install --source dist --version 0.2.0 \
        --platform windows-x86_64 --root <install root>
"""

from __future__ import annotations

import argparse
import datetime as dt
import sys
import tempfile
from pathlib import Path

from . import keys, layout, manifest as manifest_module, package, publish, verify

TRUSTED_KEYS = Path(__file__).resolve().parents[2] / "release" / "trusted-keys.json"


def _platforms(values: list[str]) -> dict[str, Path]:
    directories = {}
    for value in values:
        if "=" not in value:
            raise SystemExit(f"--platform は <name>=<dir>: {value!r}")
        name, directory = value.split("=", 1)
        if name in directories:
            raise SystemExit(f"platform が重複: {name!r}")
        directories[name] = Path(directory)
    if not directories:
        raise SystemExit("--platform が 1 つもない")
    return directories


def build(args) -> int:
    key_id, seed = keys.load_signing_key(args.key)
    work = Path(tempfile.mkdtemp(prefix="lapis-package-"))
    packages = {}
    for platform, directory in _platforms(args.platform).items():
        packages[platform] = package.build(
            directory, work / f"{args.name}-{args.version}-{platform}.zip"
        )
    generated_at = args.generated_at or dt.datetime.now(dt.timezone.utc).replace(
        microsecond=0
    ).isoformat().replace("+00:00", "Z")
    manifest = manifest_module.build(
        version=args.version,
        packages=packages,
        notes_url=args.notes_url,
        breaking=args.breaking,
        key_id=key_id,
        generated_at=generated_at,
        signing_key=seed,
    )
    out = Path(args.out)
    if out.exists() and any(out.iterdir()):
        raise SystemExit(f"出力先が空でない: {out}")
    out.mkdir(parents=True, exist_ok=True)
    for result in packages.values():
        (out / result.name).write_bytes(result.read_bytes())
    (out / publish.MANIFEST_NAME).write_bytes(manifest_module.dump(manifest))
    print(f"生成: {out}")
    for path in sorted(out.iterdir()):
        print(f"  {path.name}")
    return 0


def _trusted(args) -> dict[str, str]:
    return keys.load_trusted_keys(Path(args.trusted_keys))


def publish_command(args) -> int:
    artifacts = publish.Artifacts(
        directory=Path(args.source), trusted_keys=_trusted(args)
    )
    if bool(args.local) == bool(args.github):
        raise SystemExit("--local か --github のどちらか一方を指定する")
    if args.local:
        destination = publish.publish_local(artifacts, Path(args.local))
        verified = verify.verify_distribution(destination, artifacts.trusted_keys)
        print(f"配置: {destination} (version {verified.version})")
        return 0
    if not args.tag:
        raise SystemExit("--github には --tag が必要")
    if args.execute and args.create and not args.notes:
        raise SystemExit("--create には --notes (release notes file) が必要")
    command = publish.publish_github_releases(
        artifacts,
        tag=args.tag,
        repo=args.repo,
        remote=publish.GhRemote(args.repo),
        execute=args.execute,
        allow_create=args.create,
        title=args.title or f"Lapis {args.tag}",
        notes_file=Path(args.notes) if args.notes else None,
    )
    print(" ".join(command))
    if not args.execute:
        print("dry-run: GitHub へは書き込んでいない (--execute で実行)")
        if args.create:
            print("--execute 時は、既存 tag に対する Release が無ければ作成する")
    return 0


def verify_command(args) -> int:
    target = Path(args.source)
    trusted = _trusted(args)
    if layout.pointer_path(target).exists():
        result = verify.verify_install(target, trusted)
        print(f"install OK: {result.version} (breaking={result.breaking})")
    else:
        result = verify.verify_distribution(target, trusted)
        print(f"manifest OK: {result.version} (breaking={result.breaking})")
    for platform, path in sorted(result.packages.items()):
        print(f"  {platform}: {path.name}")
    return 0


def install_command(args) -> int:
    target = layout.install(
        Path(args.root),
        Path(args.source),
        version=args.version,
        platform=args.platform,
        trusted_keys=_trusted(args),
        replace=args.replace,
    )
    print(f"install OK: {target}")
    return 0


def activate_command(args) -> int:
    target = layout.activate(
        Path(args.root),
        args.version,
        platform=args.platform,
        trusted_keys=_trusted(args),
    )
    print(f"activate OK: {target}")
    return 0


def keygen_command(args) -> int:
    destination = Path(args.out)
    keys.warn_if_inside_repository(destination)
    public = keys.generate(args.key_id, destination)
    print(f"key_id: {args.key_id}")
    print(f"public: {public}")
    print("公開鍵を release/trusted-keys.json へ追加する。秘密鍵は repo へ置かない")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    build_parser = sub.add_parser("build", help="package と署名付き manifest を生成する")
    build_parser.add_argument("--version", required=True)
    build_parser.add_argument(
        "--platform", action="append", default=[], help="<name>=<dir>"
    )
    build_parser.add_argument("--name", default="lapis", help="package 名の接頭辞")
    build_parser.add_argument("--out", required=True)
    build_parser.add_argument("--notes-url", required=True)
    build_parser.add_argument("--breaking", action="store_true")
    build_parser.add_argument("--generated-at")
    build_parser.add_argument("--key")
    build_parser.set_defaults(handler=build)

    publish_parser = sub.add_parser("publish", help="生成物を配置する")
    publish_parser.add_argument("--source", required=True)
    publish_parser.add_argument("--local")
    publish_parser.add_argument("--github", action="store_true")
    publish_parser.add_argument("--tag", help="既に存在する tag")
    publish_parser.add_argument("--title")
    publish_parser.add_argument("--notes")
    publish_parser.add_argument(
        "--create",
        action="store_true",
        help="tag に対応する Release が無い場合に作成する（tag は作らない）",
    )
    publish_parser.add_argument("--repo", default="yuki-net/lapis")
    publish_parser.add_argument("--trusted-keys", default=str(TRUSTED_KEYS))
    publish_parser.add_argument(
        "--execute", action="store_true", help="GitHub へ実際に upload する"
    )
    publish_parser.set_defaults(handler=publish_command)

    verify_parser = sub.add_parser("verify", help="配置物と layout を検証する")
    verify_parser.add_argument("--source", required=True)
    verify_parser.add_argument("--trusted-keys", default=str(TRUSTED_KEYS))
    verify_parser.set_defaults(handler=verify_command)

    install_parser = sub.add_parser("install", help="配置物を layout へ適用する")
    install_parser.add_argument("--source", required=True)
    install_parser.add_argument("--root", required=True)
    install_parser.add_argument("--version", required=True)
    install_parser.add_argument("--platform", required=True)
    install_parser.add_argument(
        "--replace", action="store_true", help="同じ version を検証して入れ直す"
    )
    install_parser.add_argument("--trusted-keys", default=str(TRUSTED_KEYS))
    install_parser.set_defaults(handler=install_command)

    activate_parser = sub.add_parser(
        "activate", help="展開済みの version へ pointer を戻す（手動復旧）"
    )
    activate_parser.add_argument("--root", required=True)
    activate_parser.add_argument("--version", required=True)
    activate_parser.add_argument("--platform", required=True)
    activate_parser.add_argument("--trusted-keys", default=str(TRUSTED_KEYS))
    activate_parser.set_defaults(handler=activate_command)

    keygen_parser = sub.add_parser("keygen", help="署名鍵を生成する")
    keygen_parser.add_argument("--key-id", required=True)
    keygen_parser.add_argument("--out", required=True)
    keygen_parser.set_defaults(handler=keygen_command)

    args = parser.parse_args(argv)
    try:
        return args.handler(args)
    except (ValueError, OSError, RuntimeError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
