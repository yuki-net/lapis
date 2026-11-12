"""配布 package・署名付き manifest の生成から配置・検証までを確認する。

ローカル配置と GitHub Releases 配置は同じ成果物と同じ検証経路を使う。
Release への書き込みは gh CLI 呼び出しを差し替えて、upload の順序だけを確認する。
"""

import hashlib
import http.server
import io
import json
import sys
import tempfile
import threading
import unittest
import zipfile
from functools import partial
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent / "package"))

import distribution as tool


def make_binaries(directory: Path, marker: bytes = b"v1") -> Path:
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "lapis.exe").write_bytes(b"MZ desktop " + marker)
    (directory / "lapis-host.exe").write_bytes(b"MZ host " + marker)
    (directory / "ignored.pdb").write_bytes(b"debug symbols")
    return directory


class KeyTests(unittest.TestCase):
    def test_keygen_writes_32_byte_hex_pair_and_refuses_overwrite(self):
        with tempfile.TemporaryDirectory() as directory:
            private = Path(directory) / "signing.key"
            public = Path(directory) / "manifest.pub"
            tool.write_keypair(private, public)
            self.assertEqual(len(bytes.fromhex(private.read_text(encoding="ascii"))), 32)
            self.assertEqual(len(bytes.fromhex(public.read_text(encoding="ascii"))), 32)
            private_key = tool.load_private_key(private)
            self.assertEqual(tool.public_key_hex(private_key), public.read_text(encoding="ascii").strip())
            with self.assertRaises(tool.DistributionError):
                tool.write_keypair(private, public)

    def test_public_key_accepts_hex_and_path_but_rejects_wrong_length(self):
        with tempfile.TemporaryDirectory() as directory:
            private = Path(directory) / "signing.key"
            public = Path(directory) / "manifest.pub"
            tool.write_keypair(private, public)
            text = public.read_text(encoding="ascii").strip()
            self.assertIsNotNone(tool.load_public_key(text))
            self.assertIsNotNone(tool.load_public_key(str(public)))
            with self.assertRaises(tool.DistributionError):
                tool.load_public_key("00" * 31)


class ManifestTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        tool.write_keypair(self.root / "signing.key", self.root / "manifest.pub")
        self.private_key = tool.load_private_key(self.root / "signing.key")
        self.public_key = tool.load_public_key(str(self.root / "manifest.pub"))

    def payload(self, **overrides):
        payload = {
            "schema": tool.SCHEMA_VERSION,
            "version": "0.1.0",
            "breaking": False,
            "release_notes_url": "https://example.invalid/releases/v0.1.0",
            "assets": {
                "windows-x86_64": {
                    "asset": "https://example.invalid/lapis-0.1.0-windows-x86_64.zip",
                    "sha256": "a" * 64,
                    "size": 1,
                }
            },
        }
        payload.update(overrides)
        return payload

    def test_schema_is_fixed_and_signed(self):
        manifest = tool.sign_manifest(self.payload(), self.private_key, "test-key")
        self.assertEqual(
            set(manifest),
            {"schema", "version", "breaking", "release_notes_url", "assets", "signature"},
        )
        self.assertEqual(manifest["signature"]["algorithm"], "ed25519")
        self.assertEqual(manifest["signature"]["key_id"], "test-key")
        self.assertFalse(manifest["breaking"])
        verified = tool.verify_manifest(json.loads(tool.manifest_bytes(manifest)), self.public_key)
        self.assertEqual(verified["release_notes_url"], "https://example.invalid/releases/v0.1.0")

    def test_breaking_flag_round_trips(self):
        manifest = tool.sign_manifest(self.payload(breaking=True), self.private_key, "test-key")
        self.assertTrue(tool.verify_manifest(manifest, self.public_key)["breaking"])

    def test_tampered_payload_is_rejected(self):
        manifest = tool.sign_manifest(self.payload(), self.private_key, "test-key")
        manifest["version"] = "0.2.0"
        with self.assertRaises(tool.DistributionError):
            tool.verify_manifest(manifest, self.public_key)

    def test_other_key_is_rejected(self):
        manifest = tool.sign_manifest(self.payload(), self.private_key, "test-key")
        tool.write_keypair(self.root / "other.key", self.root / "other.pub")
        with self.assertRaises(tool.DistributionError):
            tool.verify_manifest(manifest, tool.load_public_key(str(self.root / "other.pub")))

    def test_invalid_schema_is_rejected(self):
        cases = {
            "missing item": {key: value for key, value in self.payload().items() if key != "breaking"},
            "extra item": {**self.payload(), "channel": "stable"},
            "not bool": self.payload(breaking="no"),
            "relative notes url": self.payload(release_notes_url="/releases/v0.1.0"),
            "empty assets": self.payload(assets={}),
            "asset scheme": self.payload(
                assets={"windows-x86_64": {"asset": "file:///x.zip", "sha256": "a" * 64, "size": 1}}
            ),
            "path traversal": self.payload(
                assets={
                    "windows-x86_64": {"asset": "../../other/x.zip", "sha256": "a" * 64, "size": 1}
                }
            ),
            "version separators": self.payload(version="../0.1.0"),
            "platform separators": self.payload(
                assets={
                    "windows/x86": {
                        "asset": "https://example.invalid/x.zip",
                        "sha256": "a" * 64,
                        "size": 1,
                    }
                }
            ),
        }
        for name, payload in cases.items():
            with self.subTest(name=name):
                with self.assertRaises(tool.DistributionError):
                    tool.sign_manifest(payload, self.private_key, "test-key")


class PackageTests(unittest.TestCase):
    def test_package_contains_only_binaries_and_is_reproducible(self):
        with tempfile.TemporaryDirectory() as directory:
            files = tool.collect_binaries(make_binaries(Path(directory)))
            self.assertEqual(sorted(files), ["lapis-host.exe", "lapis.exe"])
            first = tool.build_package(files)
            second = tool.build_package(tool.collect_binaries(Path(directory)))
            self.assertEqual(hashlib.sha256(first).hexdigest(), hashlib.sha256(second).hexdigest())
            with zipfile.ZipFile(io.BytesIO(first)) as archive:
                self.assertEqual(sorted(archive.namelist()), ["lapis-host.exe", "lapis.exe"])

    def test_missing_required_binary_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            make_binaries(Path(directory))
            (Path(directory) / "lapis-host.exe").unlink()
            with self.assertRaises(tool.DistributionError):
                tool.collect_binaries(Path(directory))
        with self.assertRaises(tool.DistributionError):
            tool.build_package({"lapis.exe": b"MZ"})


class LocalAndReleaseTests(unittest.TestCase):
    """test package 2版を生成 → ローカル配置 → 署名検証まで通す。"""

    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        tool.write_keypair(self.root / "signing.key", self.root / "manifest.pub")
        self.private_key = tool.load_private_key(self.root / "signing.key")
        self.public_key = tool.load_public_key(str(self.root / "manifest.pub"))
        self.binaries = make_binaries(self.root / "release", b"v1")
        self.destination = self.root / "dist"

    def publish(self, version, breaking=False, destination=None):
        return tool.publish(
            version=version,
            packages={"windows-x86_64": tool.collect_binaries(self.binaries)},
            destination=destination or tool.LocalDirectory(self.destination),
            release_notes_url=f"https://example.invalid/releases/v{version}",
            breaking=breaking,
            private_key=self.private_key,
            key_id="test-key",
        )

    def test_two_versions_publish_place_and_verify_locally(self):
        first = self.publish("0.1.0")
        self.binaries = make_binaries(self.binaries, b"v2")
        second = self.publish("0.2.0", breaking=True)

        self.assertEqual(first["assets"]["windows-x86_64"]["asset"], "lapis-0.1.0-windows-x86_64.zip")
        self.assertTrue((self.destination / "manifest.json").is_file())
        self.assertTrue((self.destination / "manifest.pub").is_file())
        self.assertNotEqual(
            (self.destination / "lapis-0.1.0-windows-x86_64.zip").read_bytes(),
            (self.destination / "lapis-0.2.0-windows-x86_64.zip").read_bytes(),
        )

        # 現在の manifest は最新版を指し、asset の sha256・size と一致する。
        verified = tool.verify(tool.LocalSource(self.destination), self.public_key)
        self.assertEqual(verified["version"], "0.2.0")
        self.assertTrue(verified["breaking"])
        asset = verified["assets"]["windows-x86_64"]
        data = (self.destination / asset["asset"]).read_bytes()
        self.assertEqual(asset["sha256"], hashlib.sha256(data).hexdigest())
        self.assertEqual(asset["size"], len(data))
        self.assertEqual(second["assets"]["windows-x86_64"], asset)

    def test_release_entry_point_uses_the_same_artifacts(self):
        """GitHub Releases 向けの参照は絶対 URL で、配置物は同じ zip と manifest。"""
        release = tool.GitHubRelease("v0.2.0", asset_base="https://example.invalid/download/v0.2.0")
        uploaded = {}

        def fake_write(self, files):
            uploaded.update(files)

        with patch.object(tool.GitHubRelease, "write", fake_write):
            manifest = self.publish("0.2.0", destination=release)
        self.assertEqual(
            manifest["assets"]["windows-x86_64"]["asset"],
            "https://example.invalid/download/v0.2.0/lapis-0.2.0-windows-x86_64.zip",
        )
        self.assertEqual(
            sorted(uploaded), ["lapis-0.2.0-windows-x86_64.zip", "manifest.json", "manifest.pub"]
        )

    def test_release_upload_puts_manifest_last(self):
        release = tool.GitHubRelease("v0.2.0", asset_base="https://example.invalid/download/v0.2.0")
        calls = []

        def fake_run(command):
            calls.append(command)
            if command[:3] == ["git", "rev-parse", "HEAD"]:
                return "abc123\n"
            return ""

        with (
            patch.object(tool, "release_exists", return_value=False),
            patch.object(tool, "tag_exists", return_value=False),
            patch.object(tool, "run", fake_run),
        ):
            release.write({"manifest.json": b"{}", "lapis-0.2.0-windows-x86_64.zip": b"zip"})

        upload = [call for call in calls if call[:3] == ["gh", "release", "upload"]][0]
        self.assertEqual(upload[3], "v0.2.0")
        self.assertTrue(upload[4].endswith("lapis-0.2.0-windows-x86_64.zip"))
        self.assertTrue(upload[-2].endswith("manifest.json"))
        self.assertIn("--clobber", upload)
        create = [call for call in calls if call[:3] == ["gh", "release", "create"]][0]
        self.assertIn("--target", create)
        self.assertEqual(create[create.index("--target") + 1], "abc123")

    def test_existing_tag_is_not_moved(self):
        """tag が既にあるときは --target を付けず、gh に既存 tag を使わせる。"""
        release = tool.GitHubRelease("v0.2.0", asset_base="https://example.invalid/download/v0.2.0")
        calls = []

        def fake_run(command):
            calls.append(command)
            return ""

        with (
            patch.object(tool, "release_exists", return_value=False),
            patch.object(tool, "tag_exists", return_value=True),
            patch.object(tool, "run", fake_run),
        ):
            release.write({"manifest.json": b"{}"})

        create = [call for call in calls if call[:3] == ["gh", "release", "create"]][0]
        self.assertNotIn("--target", create)

    def test_same_artifacts_verify_over_http(self):
        """静的配置（HTTP）でもローカルフォルダと同じ検証経路で通る。"""
        self.publish("0.2.0")
        handler = partial(QuietHandler, directory=str(self.destination))
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        base = f"http://127.0.0.1:{server.server_address[1]}/"

        payload = tool.verify(tool.UrlSource(base), self.public_key)
        self.assertEqual(payload["version"], "0.2.0")
        # 相対パスの manifest は取得元を基準に asset を解決する。
        self.assertTrue(payload["assets"]["windows-x86_64"]["asset"].startswith("lapis-"))

    def test_tampered_asset_fails_verification(self):
        self.publish("0.2.0")
        asset = self.destination / "lapis-0.2.0-windows-x86_64.zip"
        asset.write_bytes(asset.read_bytes() + b"tampered")
        with self.assertRaises(tool.DistributionError):
            tool.verify(tool.LocalSource(self.destination), self.public_key)

    def test_path_outside_distribution_folder_is_rejected(self):
        self.publish("0.2.0")
        source = tool.LocalSource(self.destination)
        with self.assertRaises(tool.DistributionError):
            source.read("../signing.key")
        with self.assertRaises(tool.DistributionError):
            tool.UrlSource("https://example.invalid/lapis/stable").read("../../other/manifest.json")

    def test_asset_base_builds_absolute_reference_for_a_relative_folder(self):
        destination = tool.LocalDirectory(self.destination, "https://example.invalid/lapis/stable")
        manifest = self.publish("0.2.0", destination=destination)
        self.assertEqual(
            manifest["assets"]["windows-x86_64"]["asset"],
            "https://example.invalid/lapis/stable/lapis-0.2.0-windows-x86_64.zip",
        )


class QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, format, *args):
        pass


class WorkflowTests(unittest.TestCase):
    """CI が同じスクリプトと同じ依存で動くことを固定する。"""

    def workflow(self, name):
        return (Path(__file__).resolve().parents[1] / ".github" / "workflows" / name).read_text(
            encoding="utf-8"
        )

    def test_repository_job_installs_packaging_dependencies(self):
        workflow = self.workflow("ci.yml")
        repository = workflow.split("\n  repository:\n", 1)[1].split("\n  rust_core:\n", 1)[0]
        self.assertIn("scripts/package/requirements.txt", repository)
        self.assertIn("python scripts/check.py repository", repository)

    def test_release_workflow_uses_the_same_publish_command(self):
        workflow = self.workflow("release.yml")
        self.assertIn("scripts/package/distribution.py", workflow)
        self.assertIn('"publish"', workflow)
        self.assertIn("github:$env:TAG", workflow)
        self.assertIn("secrets.LAPIS_RELEASE_SIGNING_KEY", workflow)
        # 署名鍵は secret から一時ファイルへ出し、終了時に必ず消す。
        self.assertIn("if: always()", workflow)
        # secret が無いまま署名鍵なしで公開しない。
        self.assertIn("throw", workflow)


    def test_platform_option_parsing_and_duplicates(self):
        with tempfile.TemporaryDirectory() as directory:
            binaries = make_binaries(Path(directory) / "one", b"v1")
            other = make_binaries(Path(directory) / "two", b"v2")
            packages = tool.packages_from_arguments([str(binaries), f"linux-x86_64={other}"], [])
            self.assertEqual(sorted(packages), ["linux-x86_64", tool.DEFAULT_PLATFORM])
            with self.assertRaises(tool.DistributionError):
                tool.packages_from_arguments([str(binaries), f"{tool.DEFAULT_PLATFORM}={other}"], [])
            with self.assertRaises(tool.DistributionError):
                tool.packages_from_arguments([], [])


class CliTests(unittest.TestCase):
    def test_publish_and_verify_round_trip_from_command_line(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binaries = make_binaries(root / "release")
            tool.write_keypair(root / "signing.key", root / "manifest.pub")
            self.assertEqual(
                tool.main(
                    [
                        "publish",
                        "--version", "0.3.0",
                        "--binaries", str(binaries),
                        "--dest", f"local:{root / 'dist'}",
                        "--release-notes-url", "https://example.invalid/releases/v0.3.0",
                        "--private-key", str(root / "signing.key"),
                        "--output", str(root / "out"),
                    ]
                ),
                0,
            )
            self.assertTrue((root / "out" / "manifest.json").is_file())
            self.assertEqual(
                tool.main(
                    [
                        "verify",
                        "--root", f"local:{root / 'dist'}",
                        "--public-key", str(root / "manifest.pub"),
                    ]
                ),
                0,
            )

    def test_invalid_destination_is_reported(self):
        self.assertEqual(tool.main(["verify", "--root", "ftp:/x", "--public-key", "00" * 32]), 1)


if __name__ == "__main__":
    unittest.main()
