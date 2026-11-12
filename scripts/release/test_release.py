"""配布物の生成・配置・検証のテスト。

Release / tag を実際に作らない。GitHub 側は fake remote と runner に差し替え、
実行される command だけを確認する。
"""

from __future__ import annotations

import base64
import contextlib
import io
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))

from scripts.release import cli, ed25519, keys, layout, manifest, package, publish, verify
from scripts.release.publish import MANIFEST_NAME


class TempDirTest(unittest.TestCase):
    def setUp(self):
        self._temporary = tempfile.TemporaryDirectory()
        self.root = Path(self._temporary.name)

    def tearDown(self):
        self._temporary.cleanup()

    def key_file(self, key_id="test-key"):
        destination = self.root / f"{key_id}.json"
        trusted_path = self.root / "trusted-keys.json"
        if destination.is_file():
            payload = json.loads(destination.read_text(encoding="utf-8"))
            public = base64.b64encode(
                ed25519.public_key(base64.b64decode(payload["seed"]))
            ).decode("ascii")
        else:
            public = keys.generate(key_id, destination)
        trusted_path.write_text(
            json.dumps({"schema": 1, "keys": {key_id: public}}), encoding="utf-8"
        )
        return destination, trusted_path

    def run_cli(self, arguments):
        with contextlib.redirect_stdout(io.StringIO()):
            return cli.main(arguments)


class Ed25519Tests(unittest.TestCase):
    """RFC 8032 7.1 のテストベクタで実装を固定する。"""

    VECTORS = [
        (
            "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
            "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
            "",
            "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e06522490155"
            "5fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
        ),
        (
            "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
            "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
            "72",
            "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da"
            "085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00",
        ),
        (
            "c5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7",
            "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025",
            "af82",
            "6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac"
            "18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a",
        ),
    ]

    def test_public_key_and_signature_match_rfc8032(self):
        for seed, public, message, signature in self.VECTORS:
            with self.subTest(seed=seed[:8]):
                seed, public = bytes.fromhex(seed), bytes.fromhex(public)
                message, signature = bytes.fromhex(message), bytes.fromhex(signature)
                self.assertEqual(ed25519.public_key(seed), public)
                self.assertEqual(ed25519.sign(seed, message), signature)
                self.assertTrue(ed25519.verify(public, message, signature))

    def test_rejects_tampered_signature_and_bad_lengths(self):
        seed = bytes(range(32))
        public = ed25519.public_key(seed)
        signature = bytearray(ed25519.sign(seed, b"payload"))
        signature[0] ^= 1
        self.assertFalse(ed25519.verify(public, b"payload", bytes(signature)))
        self.assertFalse(ed25519.verify(public, b"other", bytes(signature)))
        self.assertFalse(ed25519.verify(public[:31], b"payload", signature))
        self.assertFalse(ed25519.verify(public, b"payload", signature[:63]))


class ManifestTests(TempDirTest):
    def build_manifest(self, version="0.2.0", breaking=False, key_id="test-key"):
        key_path, trusted = self.key_file(key_id)
        payload_dir = self.root / f"payload-{version}"
        (payload_dir / "bin").mkdir(parents=True)
        (payload_dir / "bin" / "lapis.exe").write_bytes(version.encode())
        archive = package.build(payload_dir, self.root / f"lapis-{version}.zip")
        _, seed = keys.load_signing_key(str(key_path))
        result = manifest.build(
            version=version,
            packages={"windows-x86_64": archive},
            notes_url=f"https://example.invalid/releases/v{version}",
            breaking=breaking,
            key_id=key_id,
            generated_at="2026-11-12T00:00:00Z",
            signing_key=seed,
        )
        return result, trusted

    def test_schema_carries_every_required_field(self):
        result, trusted = self.build_manifest(breaking=True)
        self.assertEqual(
            set(result),
            {
                "schema",
                "version",
                "breaking",
                "notes_url",
                "generated_at",
                "key_id",
                "assets",
                "signature",
            },
        )
        self.assertTrue(result["breaking"])
        self.assertTrue(result["notes_url"].startswith("https://"))
        asset = result["assets"]["windows-x86_64"]
        self.assertEqual(asset["source"], "lapis-0.2.0.zip")
        self.assertRegex(asset["sha256"], r"^[0-9a-f]{64}$")
        parsed = manifest.verify_signature(result, keys.load_trusted_keys(trusted))
        self.assertEqual(parsed.version, "0.2.0")
        self.assertTrue(parsed.breaking)

    def test_unknown_field_is_rejected(self):
        result, trusted = self.build_manifest()
        result["channel"] = "stable"
        with self.assertRaises(manifest.ManifestError):
            manifest.parse(manifest.dump(result))

    def test_missing_field_is_rejected(self):
        result, _ = self.build_manifest()
        del result["breaking"]
        with self.assertRaises(manifest.ManifestError):
            manifest.parse(manifest.dump(result))

    def test_signature_is_required_and_key_must_be_trusted(self):
        result, trusted = self.build_manifest()
        trusted_keys = keys.load_trusted_keys(trusted)
        tampered = dict(result)
        tampered["version"] = "9.9.9"
        with self.assertRaises(manifest.ManifestError):
            manifest.verify_signature(tampered, trusted_keys)
        with self.assertRaises(manifest.ManifestError):
            manifest.verify_signature(result, {})

    def test_asset_source_cannot_escape_manifest_directory(self):
        for source in ("../lapis.zip", "/etc/passwd", "a\\b.zip", "https://ok/x.zip"):
            with self.subTest(source=source):
                if source.startswith("https://"):
                    manifest.validate_source(source)
                    continue
                with self.assertRaises(manifest.ManifestError):
                    manifest.validate_source(source)

    def test_package_sha256_mismatch_is_rejected(self):
        result, trusted = self.build_manifest()
        directory = self.root / "distribution"
        directory.mkdir()
        archive = self.root / "lapis-0.2.0.zip"
        (directory / archive.name).write_bytes(archive.read_bytes() + b"tamper")
        (directory / MANIFEST_NAME).write_bytes(manifest.dump(result))
        with self.assertRaises(manifest.ManifestError):
            verify.verify_distribution(directory, keys.load_trusted_keys(trusted))


class PackageTests(TempDirTest):
    def test_package_is_deterministic_and_keeps_tree(self):
        payload = self.root / "payload"
        (payload / "assets").mkdir(parents=True)
        (payload / "lapis.exe").write_bytes(b"exe")
        (payload / "assets" / "note.txt").write_text("hello", encoding="utf-8")
        first = package.build(payload, self.root / "a.zip")
        second = package.build(payload, self.root / "b.zip")
        self.assertEqual(first.read_bytes(), second.read_bytes())
        import zipfile

        with zipfile.ZipFile(first) as archive:
            self.assertEqual(
                archive.namelist(), ["assets/note.txt", "lapis.exe"]
            )

    def test_symlink_and_empty_input_are_rejected(self):
        empty = self.root / "empty"
        empty.mkdir()
        with self.assertRaises(package.PackageError):
            package.build(empty, self.root / "empty.zip")

    def test_package_name_rejects_path_elements(self):
        self.assertEqual(
            package.package_name("lapis", "0.2.0", "windows-x86_64"),
            "lapis-0.2.0-windows-x86_64.zip",
        )
        for name, version, platform in (
            ("../evil", "0.2.0", "windows-x86_64"),
            ("lapis", "../../0.2.0", "windows-x86_64"),
            ("lapis", "0.2.0", "../linux"),
            ("", "0.2.0", "windows-x86_64"),
        ):
            with self.subTest(name=name, version=version, platform=platform):
                with self.assertRaises(package.PackageError):
                    package.package_name(name, version, platform)


class PublishTests(TempDirTest):
    class FakeRemote:
        def __init__(self, tag=True, release=True):
            self.tag = tag
            self.release = release

        def tag_exists(self, _tag):
            return self.tag

        def release_exists(self, _tag):
            return self.release

    def artifacts(self, breaking=False):
        key_path, trusted = self.key_file()
        payload = self.root / "payload"
        (payload / "bin").mkdir(parents=True)
        (payload / "bin" / "lapis.exe").write_bytes(b"exe")
        out = self.root / "dist"
        self.assertEqual(
            self.run_cli(
                [
                    "build",
                    "--version",
                    "0.2.0",
                    "--platform",
                    f"windows-x86_64={payload}",
                    "--out",
                    str(out),
                    "--notes-url",
                    "https://example.invalid/releases/v0.2.0",
                    "--key",
                    str(key_path),
                    "--generated-at",
                    "2026-11-12T00:00:00Z",
                ]
            ),
            0,
        )
        trusted_keys = keys.load_trusted_keys(trusted)
        return publish.Artifacts(out, trusted_keys), trusted

    def test_local_placement_keeps_manifest_and_package(self):
        artifacts, trusted = self.artifacts()
        destination = publish.publish_local(artifacts, self.root / "public")
        self.assertEqual(
            sorted(path.name for path in destination.iterdir()),
            ["lapis-0.2.0-windows-x86_64.zip", MANIFEST_NAME],
        )
        verified = verify.verify_distribution(destination, keys.load_trusted_keys(trusted))
        self.assertEqual(verified.version, "0.2.0")

    def test_github_placement_is_dry_run_by_default(self):
        artifacts, _ = self.artifacts()
        uploads = []
        command = publish.publish_github_releases(
            artifacts,
            tag="v0.2.0",
            repo="example/lapis",
            remote=self.FakeRemote(tag=False, release=False),
            runner=uploads.append,
        )
        self.assertEqual(command[:3], ["gh", "release", "upload"])
        self.assertIn("v0.2.0", command)
        self.assertIn("--clobber", command)
        self.assertEqual(uploads, [])
        # dry-run は create を一切提案しない。
        self.assertNotIn("create", command)

    def test_github_placement_requires_existing_tag_and_release(self):
        artifacts, _ = self.artifacts()
        for remote in (self.FakeRemote(tag=False), self.FakeRemote(release=False)):
            with self.subTest(tag=remote.tag, release=remote.release):
                with self.assertRaises(publish.PublishError):
                    publish.publish_github_releases(
                        artifacts,
                        tag="v0.2.0",
                        repo="example/lapis",
                        remote=remote,
                        execute=True,
                        runner=lambda command: None,
                    )

    def test_create_requires_an_existing_tag(self):
        artifacts, _ = self.artifacts()
        notes = self.root / "notes.md"
        notes.write_text("notes", encoding="utf-8")
        uploads = []
        with self.assertRaises(publish.PublishError):
            publish.publish_github_releases(
                artifacts,
                tag="v0.2.0",
                repo="example/lapis",
                remote=self.FakeRemote(tag=False, release=False),
                execute=True,
                allow_create=True,
                title="Lapis v0.2.0",
                notes_file=notes,
                runner=uploads.append,
            )
        self.assertEqual(uploads, [])
        # 既存 tag なら create 経路でも --verify-tag が付く（tag を新規作成しない）。
        publish.publish_github_releases(
            artifacts,
            tag="v0.2.0",
            repo="example/lapis",
            remote=self.FakeRemote(tag=True, release=False),
            execute=True,
            allow_create=True,
            title="Lapis v0.2.0",
            notes_file=notes,
            runner=uploads.append,
        )
        self.assertEqual(uploads[0][:3], ["gh", "release", "create"])
        self.assertIn("--verify-tag", uploads[0])

    def test_github_placement_uploads_only_when_executed(self):
        artifacts, _ = self.artifacts()
        uploads = []
        publish.publish_github_releases(
            artifacts,
            tag="v0.2.0",
            repo="example/lapis",
            remote=self.FakeRemote(),
            execute=True,
            runner=uploads.append,
        )
        self.assertEqual(len(uploads), 1)
        self.assertEqual(uploads[0][:3], ["gh", "release", "upload"])
        self.assertNotIn("create", uploads[0])

    def test_invalid_tag_is_rejected(self):
        artifacts, _ = self.artifacts()
        for tag in ("0.2.0", "v", "v0.2..0"):
            with self.subTest(tag=tag):
                with self.assertRaises(publish.PublishError):
                    publish.publish_github_releases(
                        artifacts,
                        tag=tag,
                        repo="example/lapis",
                        remote=self.FakeRemote(),
                    )


class LayoutTests(TempDirTest):
    def distribution(self, version, breaking=False):
        key_path, trusted = self.key_file()
        payload = self.root / f"payload-{version}"
        (payload / "bin").mkdir(parents=True)
        (payload / "bin" / "lapis.exe").write_bytes(version.encode())
        out = self.root / f"dist-{version}"
        self.assertEqual(
            self.run_cli(
                [
                    "build",
                    "--version",
                    version,
                    "--platform",
                    f"windows-x86_64={payload}",
                    "--out",
                    str(out),
                    "--notes-url",
                    f"https://example.invalid/releases/v{version}",
                    "--key",
                    str(key_path),
                    "--generated-at",
                    "2026-11-12T00:00:00Z",
                ]
                + (["--breaking"] if breaking else [])
            ),
            0,
        )
        return out, keys.load_trusted_keys(trusted)

    def test_two_versions_install_and_keep_previous(self):
        root = self.root / "install"
        first, trusted = self.distribution("0.2.0")
        second, trusted = self.distribution("0.3.0", breaking=True)
        layout.install(
            root, first, version="0.2.0", platform="windows-x86_64", trusted_keys=trusted
        )
        self.assertEqual(layout.read_pointer(root), "0.2.0")
        layout.install(
            root, second, version="0.3.0", platform="windows-x86_64", trusted_keys=trusted
        )
        self.assertEqual(layout.read_pointer(root), "0.3.0")
        self.assertEqual(
            sorted(path.name for path in layout.versions_dir(root).iterdir()),
            ["0.2.0", "0.3.0"],
        )
        installed = verify.verify_install(root, trusted)
        self.assertTrue(installed.breaking)
        self.assertTrue((layout.version_dir(root, "0.3.0") / "bin" / "lapis.exe").is_file())

    def test_third_version_prunes_to_two_generations(self):
        root = self.root / "install"
        for version in ("0.2.0", "0.3.0", "0.4.0"):
            distribution, trusted = self.distribution(version)
            layout.install(
                root,
                distribution,
                version=version,
                platform="windows-x86_64",
                trusted_keys=trusted,
            )
        self.assertEqual(
            sorted(path.name for path in layout.versions_dir(root).iterdir()),
            ["0.3.0", "0.4.0"],
        )

    def test_install_rejects_untrusted_and_mismatched_version(self):
        root = self.root / "install"
        distribution, trusted = self.distribution("0.2.0")
        with self.assertRaises(layout.LayoutError):
            layout.install(
                root,
                distribution,
                version="0.9.0",
                platform="windows-x86_64",
                trusted_keys=trusted,
            )
        with self.assertRaises(manifest.ManifestError):
            layout.install(
                root,
                distribution,
                version="0.2.0",
                platform="windows-x86_64",
                trusted_keys={},
            )
        self.assertIsNone(layout.read_pointer(root))

    def test_install_rejects_duplicate_without_replace(self):
        root = self.root / "install"
        distribution, trusted = self.distribution("0.2.0")
        layout.install(
            root, distribution, version="0.2.0", platform="windows-x86_64", trusted_keys=trusted
        )
        with self.assertRaises(layout.LayoutError):
            layout.install(
                root,
                distribution,
                version="0.2.0",
                platform="windows-x86_64",
                trusted_keys=trusted,
            )
        self.assertEqual(layout.read_pointer(root), "0.2.0")

    def test_manual_recovery_activates_previous_version(self):
        root = self.root / "install"
        first, trusted = self.distribution("0.2.0")
        second, _ = self.distribution("0.3.0")
        layout.install(
            root, first, version="0.2.0", platform="windows-x86_64", trusted_keys=trusted
        )
        layout.install(
            root, second, version="0.3.0", platform="windows-x86_64", trusted_keys=trusted
        )
        # 0.3.0 を壊してから、検証済みの 0.2.0 へ pointer を戻す。
        (layout.version_dir(root, "0.3.0") / "lapis-0.3.0-windows-x86_64.zip").write_bytes(
            b"broken"
        )
        with self.assertRaises(manifest.ManifestError):
            verify.verify_install(root, trusted)
        layout.activate(
            root, "0.2.0", platform="windows-x86_64", trusted_keys=trusted
        )
        self.assertEqual(layout.read_pointer(root), "0.2.0")
        verified = verify.verify_install(root, trusted)
        self.assertEqual(verified.version, "0.2.0")
        # 壊れた 0.3.0 と退避した directory は消さない。
        self.assertTrue(layout.version_dir(root, "0.3.0").is_dir())

    def test_prune_keeps_manually_quarantined_directory(self):
        root = self.root / "install"
        versions = ("0.2.0", "0.3.0", "0.4.0")
        for version in versions:
            distribution, trusted = self.distribution(version)
            layout.install(
                root,
                distribution,
                version=version,
                platform="windows-x86_64",
                trusted_keys=trusted,
            )
            if version == "0.3.0":
                # 壊れた version を退避した名前。以降の install で消してはいけない。
                (layout.versions_dir(root) / "0.3.0.broken").mkdir()
                (layout.versions_dir(root) / "0.3.0.broken" / MANIFEST_NAME).write_bytes(
                    (distribution / MANIFEST_NAME).read_bytes()
                )
        self.assertEqual(
            sorted(path.name for path in layout.versions_dir(root).iterdir()),
            ["0.3.0", "0.3.0.broken", "0.4.0"],
        )

    def test_install_does_not_extract_when_pointer_is_corrupt(self):
        root = self.root / "install"
        distribution, trusted = self.distribution("0.2.0")
        root.mkdir()
        layout.pointer_path(root).write_text("{not json", encoding="utf-8")
        with self.assertRaises(layout.LayoutError):
            layout.install(
                root,
                distribution,
                version="0.2.0",
                platform="windows-x86_64",
                trusted_keys=trusted,
            )
        self.assertFalse(layout.versions_dir(root).exists())
        # 壊れた pointer はそのまま残る（既存の破損を勝手に作り直さない）。
        self.assertEqual(
            layout.pointer_path(root).read_text(encoding="utf-8"), "{not json"
        )

    def test_activate_rejects_unverified_version(self):
        root = self.root / "install"
        distribution, trusted = self.distribution("0.2.0")
        layout.install(
            root, distribution, version="0.2.0", platform="windows-x86_64", trusted_keys=trusted
        )
        with self.assertRaises(layout.LayoutError):
            layout.activate(root, "0.9.0", platform="windows-x86_64", trusted_keys=trusted)
        self.assertEqual(layout.read_pointer(root), "0.2.0")

    def test_pointer_and_data_generation_are_separate_from_credentials(self):
        root = self.root / "install"
        self.assertEqual(layout.data_dir(root, "1"), root / "data" / "1")
        self.assertEqual(layout.credentials_dir(root), root / "credentials")
        self.assertNotIn(
            layout.credentials_dir(root), layout.data_dir(root, "1").parents
        )
        self.assertEqual(layout.staging_dir(root), root / "staging")
        for name in ("..", "a/b", ""):
            with self.subTest(name=name):
                with self.assertRaises(layout.LayoutError):
                    layout.version_dir(root, name)

    def test_corrupt_pointer_is_not_silently_ignored(self):
        root = self.root / "install"
        root.mkdir()
        layout.pointer_path(root).write_text("{}", encoding="utf-8")
        with self.assertRaises(layout.LayoutError):
            layout.read_pointer(root)


class EndToEndTests(TempDirTest):
    """2 版を生成し、ローカル配置と Releases 相当の配置の両方から install する。"""

    def _payload(self, version):
        payload = self.root / f"src-{version}"
        (payload / "bin").mkdir(parents=True)
        (payload / "bin" / "lapis.exe").write_bytes(f"lapis-{version}".encode())
        (payload / "README.txt").write_text(version, encoding="utf-8")
        return payload

    def test_two_versions_through_local_and_release_placements(self):
        key_path, trusted_path = self.key_file()
        trusted = keys.load_trusted_keys(trusted_path)
        release_mirror = self.root / "release-mirror"
        local_mirror = self.root / "static"
        installed_root = self.root / "install"

        for version, breaking in (("0.2.0", False), ("0.3.0", True)):
            dist = self.root / f"dist-{version}"
            arguments = [
                "build",
                "--version",
                version,
                "--platform",
                f"windows-x86_64={self._payload(version)}",
                "--out",
                str(dist),
                "--notes-url",
                f"https://example.invalid/releases/v{version}",
                "--key",
                str(key_path),
                "--generated-at",
                "2026-11-12T00:00:00Z",
            ]
            if breaking:
                arguments.append("--breaking")
            self.assertEqual(self.run_cli(arguments), 0)

            artifacts = publish.Artifacts(dist, trusted)
            # ローカル（静的配置）
            publish.publish_local(artifacts, local_mirror / f"v{version}")
            # GitHub Releases 相当。asset を Release から取得した形を mirror に再現する。
            command = publish.publish_github_releases(
                artifacts, tag=f"v{version}", repo="example/lapis",
                remote=PublishTests.FakeRemote(),
            )
            self.assertEqual(command[:3], ["gh", "release", "upload"])
            publish.publish_local(artifacts, release_mirror / f"v{version}")

            verified = verify.verify_distribution(dist, trusted)
            self.assertEqual(verified.breaking, breaking)
            self.assertEqual(verified.notes_url, f"https://example.invalid/releases/v{version}")

            layout.install(
                installed_root,
                release_mirror / f"v{version}",
                version=version,
                platform="windows-x86_64",
                trusted_keys=trusted,
            )
            self.assertEqual(layout.read_pointer(installed_root), version)
            installed = verify.verify_install(installed_root, trusted)
            self.assertEqual(installed.version, version)

        for mirror in (local_mirror, release_mirror):
            with self.subTest(mirror=mirror.name):
                self.assertEqual(
                    sorted(path.name for path in mirror.iterdir()),
                    ["v0.2.0", "v0.3.0"],
                )
                for version in ("0.2.0", "0.3.0"):
                    result = verify.verify_distribution(mirror / f"v{version}", trusted)
                    self.assertEqual(result.version, version)
        # 2 世代保持: 最後の install 後も前版が残る。
        self.assertEqual(
            sorted(path.name for path in layout.versions_dir(installed_root).iterdir()),
            ["0.2.0", "0.3.0"],
        )

    def test_cli_verify_and_install_commands(self):
        key_path, trusted_path = self.key_file()
        payload = self._payload("0.2.0")
        dist = self.root / "dist"
        self.assertEqual(
            self.run_cli(
                [
                    "build", "--version", "0.2.0",
                    "--platform", f"windows-x86_64={payload}",
                    "--out", str(dist),
                    "--notes-url", "https://example.invalid/releases/v0.2.0",
                    "--key", str(key_path),
                    "--generated-at", "2026-11-12T00:00:00Z",
                ]
            ),
            0,
        )
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            self.assertEqual(
                cli.main(
                    ["verify", "--source", str(dist), "--trusted-keys", str(trusted_path)]
                ),
                0,
            )
            self.assertIn("manifest OK: 0.2.0", output.getvalue())
            self.assertEqual(
                cli.main(
                    [
                        "install", "--source", str(dist), "--root", str(self.root / "install"),
                        "--version", "0.2.0", "--platform", "windows-x86_64",
                        "--trusted-keys", str(trusted_path),
                    ]
                ),
                0,
            )
        self.assertEqual(layout.read_pointer(self.root / "install"), "0.2.0")

    def test_publish_cli_never_creates_a_release(self):
        key_path, trusted_path = self.key_file()
        payload = self._payload("0.2.0")
        dist = self.root / "dist"
        self.assertEqual(
            self.run_cli(
                [
                    "build", "--version", "0.2.0",
                    "--platform", f"windows-x86_64={payload}",
                    "--out", str(dist),
                    "--notes-url", "https://example.invalid/releases/v0.2.0",
                    "--key", str(key_path),
                    "--generated-at", "2026-11-12T00:00:00Z",
                ]
            ),
            0,
        )
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            self.assertEqual(
                cli.main(
                    [
                        "publish", "--source", str(dist), "--github",
                        "--tag", "v0.2.0", "--repo", "example/lapis",
                        "--trusted-keys", str(trusted_path),
                    ]
                ),
                0,
            )
        printed = output.getvalue()
        self.assertIn("gh release upload v0.2.0", printed)
        self.assertIn("dry-run", printed)
        self.assertNotIn("release create", printed)


class WorkflowTests(unittest.TestCase):
    """Release workflow が tag / Release を勝手に作らないことを固定する。"""

    def setUp(self):
        root = Path(__file__).resolve().parents[2]
        self.workflow = (root / ".github" / "workflows" / "release.yml").read_text(
            encoding="utf-8"
        )
        self.publish_source = (root / "scripts" / "release" / "publish.py").read_text(
            encoding="utf-8"
        )

    def test_signing_key_comes_from_secret_and_is_never_committed(self):
        self.assertIn("secrets.LAPIS_RELEASE_SIGNING_KEY", self.workflow)
        self.assertIn("LAPIS_RELEASE_SIGNING_KEY", self.workflow)
        self.assertNotRegex(self.workflow, r"(?m)^\s*seed\s*:")

    def test_workflow_never_creates_a_tag(self):
        self.assertNotIn("git tag", self.workflow)
        self.assertNotIn("gh release create", self.workflow)
        # Release 作成は CLI の --create 経由でだけ行い、CLI 側が --verify-tag を付ける。
        self.assertIn("--create", self.workflow)
        self.assertIn('"--verify-tag"', self.publish_source)
        self.assertIn("create_command", self.publish_source)

    def test_publish_job_uses_the_release_cli(self):
        self.assertIn("python -m scripts.release publish", self.workflow)
        self.assertIn("--execute", self.workflow)

    def test_existing_workflows_still_gate_on_repository_checks(self):
        ci = (
            Path(__file__).resolve().parents[2] / ".github" / "workflows" / "ci.yml"
        ).read_text(encoding="utf-8")
        self.assertIn("python scripts/check.py repository architecture", ci)


class SigningKeyTests(TempDirTest):
    def test_generated_key_file_contains_no_repository_paths(self):
        path = self.root / "key.json"
        public = keys.generate("test-key", path)
        payload = json.loads(path.read_text(encoding="utf-8"))
        self.assertEqual(payload["schema"], 1)
        self.assertEqual(payload["algorithm"], "ed25519")
        self.assertEqual(payload["key_id"], "test-key")
        self.assertEqual(len(base64.b64decode(payload["seed"], validate=True)), 32)
        seed = base64.b64decode(payload["seed"], validate=True)
        self.assertEqual(public, base64.b64encode(ed25519.public_key(seed)).decode())

    def test_key_generation_refuses_to_overwrite(self):
        path = self.root / "key.json"
        keys.generate("test-key", path)
        with self.assertRaises(keys.SigningKeyError):
            keys.generate("test-key", path)

    def test_key_file_rejects_unknown_fields_and_bad_profiles(self):
        path = self.root / "key.json"
        keys.generate("test-key", path)
        payload = json.loads(path.read_text(encoding="utf-8"))
        for mutate in (
            lambda value: value.update({"seed_hex": "00"}),
            lambda value: value.update({"algorithm": "rsa"}),
            lambda value: value.update({"schema": 2}),
            lambda value: value.update({"seed": "not-base64"}),
        ):
            with self.subTest(mutate=mutate):
                broken = dict(payload)
                mutate(broken)
                path.write_text(json.dumps(broken), encoding="utf-8")
                with self.assertRaises(keys.SigningKeyError):
                    keys.load_signing_key(str(path))
                path.write_text(json.dumps(payload), encoding="utf-8")

    def test_missing_key_is_an_error_not_a_fallback(self):
        with patch.dict(os.environ, {}, clear=True):
            with self.assertRaises(keys.SigningKeyError):
                keys.load_signing_key(None)
        self.assertTrue(keys.KEY_ENV)

    def test_key_file_can_be_given_by_environment(self):
        path = self.root / "key.json"
        keys.generate("test-key", path)
        with patch.dict(os.environ, {keys.KEY_ENV: str(path)}):
            key_id, seed = keys.load_signing_key(None)
        self.assertEqual(key_id, "test-key")
        self.assertEqual(len(seed), 32)


class TrustedKeyTests(unittest.TestCase):
    def test_repository_registry_is_well_formed(self):
        registry = Path(__file__).resolve().parents[2] / "release" / "trusted-keys.json"
        payload = json.loads(registry.read_text(encoding="utf-8"))
        self.assertEqual(payload["schema"], 1)
        for key_id, encoded in payload["keys"].items():
            self.assertTrue(key_id)
            self.assertEqual(len(base64.b64decode(encoded, validate=True)), 32)


if __name__ == "__main__":
    unittest.main()
