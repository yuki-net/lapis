import base64
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

from scripts.extensions.package import package
from scripts.extensions.check import check
from scripts.extensions.bundle import bundle
from scripts.extensions.dev_bundle import prepare
from scripts.extensions.stage import stage
from scripts.release import ed25519
from scripts.release.manifest import canonical_bytes


class PackageTests(unittest.TestCase):
    def test_stage_builds_native_adapter_without_changing_other_extensions(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, output, target = root / "extensions", root / "staged", root / "target"
            component = source / "terminal" / "component"
            component.mkdir(parents=True)
            (component / "extension.json").write_text(json.dumps({
                "id": "example.terminal", "version": "1.0.0",
                "requires": [{"contract": "lapis.extension", "minimum": "1.0.0", "maximum": "1.0.0"}],
                "files": ["extension.wasm", "views.json"]
            }), encoding="utf-8")
            (component / "Cargo.toml").write_text("", encoding="utf-8")
            (component / "views.json").write_text("{}", encoding="utf-8")
            native = source / "terminal" / "native"
            native.mkdir()
            (native / "Cargo.toml").write_text("", encoding="utf-8")
            (source / "terminal" / "native-build.json").write_text('{"manifest":"native/Cargo.toml"}', encoding="utf-8")
            artifact = target / "debug" / "terminal.dll"
            artifact.parent.mkdir(parents=True)
            artifact.write_bytes(b"native")
            with patch("scripts.extensions.stage.sys.platform", "win32"), \
                 patch("scripts.extensions.stage.platform.machine", return_value="AMD64"), \
                 patch("scripts.extensions.stage.subprocess.run") as build:
                build.return_value.stdout = json.dumps({"reason": "compiler-artifact", "manifest_path": str(native / "Cargo.toml"), "target": {"kind": ["cdylib"]}, "filenames": [str(artifact)]})
                self.assertEqual(stage(source, output, target, release=False), ["example.terminal"])
            build.assert_called_once()
            definition = json.loads((output / "terminal" / "extension.json").read_text(encoding="utf-8"))
            self.assertIn("native/windows-x86_64.dll", definition["files"])
            self.assertNotIn("extension.wasm", definition["files"])
            self.assertIn("lapis.extension.native-abi", [item["contract"] for item in definition["requires"]])
            self.assertEqual((output / "terminal" / "native/windows-x86_64.dll").read_bytes(), b"native")
            self.assertEqual((output / "terminal" / "views.json").read_text(), "{}")

    def test_stage_discovers_data_and_wasm_components(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source, output, target = root / "extensions", root / "staged", root / "target"
            data = source / "language"
            data.mkdir(parents=True)
            (data / "extension.json").write_text(json.dumps({
                "id": "example.language", "version": "1.0.0",
                "requires": [{"contract": "lapis.extension", "minimum": "1.0.0", "maximum": "1.0.0"}],
                "files": ["data.json"]
            }), encoding="utf-8")
            (data / "data.json").write_text("{}", encoding="utf-8")
            component = source / "terminal" / "component"
            component.mkdir(parents=True)
            (component / "extension.json").write_text(json.dumps({
                "id": "example.terminal", "version": "1.0.0",
                "requires": [{"contract": "lapis.extension", "minimum": "1.0.0", "maximum": "1.0.0"}],
                "files": ["extension.wasm"]
            }), encoding="utf-8")
            (component / "Cargo.toml").write_text(
                '[package]\nname="example-terminal"\n[lib]\ncrate-type=["cdylib"]\n',
                encoding="utf-8",
            )
            artifact = target / "wasm32-wasip2" / "debug" / "example_terminal.wasm"
            artifact.parent.mkdir(parents=True)
            artifact.write_bytes(b"component")
            with patch("scripts.extensions.stage.subprocess.run") as build:
                build.return_value.stdout = json.dumps({"reason": "compiler-artifact", "target": {"kind": ["cdylib"]}, "filenames": [str(artifact)]})
                self.assertEqual(stage(source, output, target, release=False),
                                 ["example.language", "example.terminal"])
            build.assert_called_once()
            self.assertEqual((output / "terminal" / "extension.wasm").read_bytes(), b"component")
            self.assertEqual((output / "language" / "data.json").read_text(), "{}")

    def test_external_example_uses_current_public_wit(self):
        root = Path(__file__).resolve().parents[2]
        self.assertEqual(
            (root / "examples/external-extension/wit/deps/lifecycle/extension.wit").read_bytes(),
            (root / "contracts/extension-api/wit/v1/extension.wit").read_bytes(),
        )
        self.assertEqual(
            (root / "examples/external-extension/wit/deps/dispatch/dispatch.wit").read_bytes(),
            (root / "contracts/extension-api/wit/dispatch-v1/dispatch.wit").read_bytes(),
        )
        for name, contract in [("lifecycle", "v1/extension.wit"),
                               ("dispatch", "dispatch-v1/dispatch.wit"),
                               ("pty", "pty-v1/pty.wit"),
                               ("process-info", "process-info-v1/process-info.wit")]:
            filename = "extension.wit" if name == "lifecycle" else f"{name}.wit"
            self.assertEqual(
                (root / "extensions/terminal/component/wit/deps" / name / filename).read_bytes(),
                (root / "contracts/extension-api/wit" / contract).read_bytes(),
            )
        self.assertEqual(
            (root / "examples/pty-extension/wit/deps/pty/pty.wit").read_bytes(),
            (root / "contracts/extension-api/wit/pty-v1/pty.wit").read_bytes(),
        )
        self.assertEqual(
            (root / "examples/pty-extension/wit/deps/dispatch/dispatch.wit").read_bytes(),
            (root / "contracts/extension-api/wit/dispatch-v1/dispatch.wit").read_bytes(),
        )

    def test_japanese_package_matches_current_bundled_messages(self):
        root = Path(__file__).resolve().parents[2]
        package = json.loads((root / "extensions/language-ja/contributions.json").read_text(encoding="utf-8"))
        messages = json.loads((root / "locales/ja-JP/messages/common.json").read_text(encoding="utf-8"))
        self.assertEqual(package["translations"][0]["messages"], messages)
        self.assertEqual(
            (root / "examples/pty-extension/wit/deps/pty/pty.wit").read_bytes(),
            (root / "contracts/extension-api/wit/pty-v1/pty.wit").read_bytes(),
        )

    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        (self.root / "extension.wasm").write_bytes(b"component")
        self.metadata = {
            "id": "dev.example.hello", "version": "1.0.0",
            "requires": [{"contract": "lapis.extension", "minimum": "1.0.0", "maximum": "1.0.0"}],
            "files": ["extension.wasm"],
        }
        self.write_metadata()

    def write_metadata(self):
        (self.root / "extension.json").write_text(json.dumps(self.metadata), encoding="utf-8")

    def test_signed_package_has_verifiable_manifest_and_matching_file(self):
        seed = bytes(range(32))
        output = self.root / "out.lapis"
        package(self.root, output, seed=seed, key_id="publisher")
        with zipfile.ZipFile(output) as archive:
            manifest = json.loads(archive.read("manifest.json"))
            signature = base64.b64decode(manifest.pop("signature"))
            self.assertTrue(ed25519.verify(ed25519.public_key(seed), canonical_bytes(manifest), signature))
            self.assertEqual(manifest["files"][0]["sha256"], hashlib.sha256(archive.read("extension.wasm")).hexdigest())

    def test_capabilities_are_signed_in_new_schema(self):
        self.metadata["capabilities"] = [{"id": "process.pty", "version": "1.0.0"}]
        self.write_metadata()
        seed = bytes(range(32))
        output = self.root / "out.lapis"
        package(self.root, output, seed=seed, key_id="publisher")
        with zipfile.ZipFile(output) as archive:
            manifest = json.loads(archive.read("manifest.json"))
        signature = base64.b64decode(manifest.pop("signature"))
        self.assertEqual(manifest["schema"], 2)
        self.assertTrue(ed25519.verify(ed25519.public_key(seed), canonical_bytes(manifest), signature))
        manifest["capabilities"][0]["version"] = "2.0.0"
        self.assertFalse(ed25519.verify(ed25519.public_key(seed), canonical_bytes(manifest), signature))

    def test_rejects_invalid_or_duplicate_capabilities(self):
        for items in ([{"id": "process.pty", "version": "0.1.0"}],
                      [{"id": "Process", "version": "1.0.0"}],
                      [{"id": "process.pty", "version": "1.0.0"}] * 2):
            self.metadata["capabilities"] = items
            self.write_metadata()
            with self.assertRaises(ValueError):
                package(self.root, self.root / "out.lapis", seed=None, key_id=None)

    def test_unsigned_package_requires_development_choice_at_call_site(self):
        output = self.root / "out.lapis"
        package(self.root, output, seed=None, key_id=None)
        with zipfile.ZipFile(output) as archive:
            manifest = json.loads(archive.read("manifest.json"))
        self.assertNotIn("signature", manifest)
        with self.assertRaises(ValueError):
            package(self.root, output, seed=bytes(32), key_id=None)

    def test_rejects_unsafe_and_case_colliding_paths_without_replacing_output(self):
        output = self.root / "out.lapis"
        output.write_bytes(b"existing")
        for paths in (["../outside"], ["manifest.json"], ["extension.wasm", "EXTENSION.WASM"]):
            self.metadata["files"] = paths
            self.write_metadata()
            with self.assertRaises(ValueError):
                package(self.root, output, seed=None, key_id=None)
            self.assertEqual(output.read_bytes(), b"existing")

    def test_rejects_symlink_and_input_as_output(self):
        with self.assertRaises(ValueError):
            package(self.root, self.root / "extension.wasm", seed=None, key_id=None)
        self.assertEqual((self.root / "extension.wasm").read_bytes(), b"component")
        link = self.root / "linked.wasm"
        try:
            link.symlink_to(self.root / "extension.wasm")
        except OSError:
            self.skipTest("symlink を作成できません")
        self.metadata["files"] = ["linked.wasm"]
        self.write_metadata()
        with self.assertRaises(ValueError):
            package(self.root, self.root / "out.lapis", seed=None, key_id=None)

    def test_check_discovers_extensions_and_rejects_duplicate_ids(self):
        extensions = self.root / "extensions"
        extensions.mkdir()
        first = extensions / "first"
        first.mkdir()
        (first / "extension.json").write_text(json.dumps(self.metadata), encoding="utf-8")
        (first / "extension.wasm").write_bytes(b"first")
        (extensions / "not-yet-packaged").mkdir()
        self.assertEqual(check(extensions), ["dev.example.hello"])
        second = extensions / "second"
        second.mkdir()
        (second / "extension.json").write_text(json.dumps(self.metadata), encoding="utf-8")
        (second / "extension.wasm").write_bytes(b"second")
        with self.assertRaises(ValueError):
            check(extensions)

    def test_release_bundle_contains_signed_discovered_extensions(self):
        extensions = self.root / "extensions"
        extensions.mkdir()
        source = extensions / "language"
        source.mkdir()
        (source / "extension.json").write_text(json.dumps(self.metadata), encoding="utf-8")
        (source / "extension.wasm").write_bytes(b"component")
        output = self.root / "bundled"
        seed = bytes(range(32))
        bundle(extensions, output, key_id="publisher", seed=seed)
        with zipfile.ZipFile(output / "0000.lapis") as archive:
            manifest = json.loads(archive.read("manifest.json"))
            signature = base64.b64decode(manifest.pop("signature"))
            self.assertTrue(ed25519.verify(ed25519.public_key(seed), canonical_bytes(manifest), signature))
            self.assertEqual(manifest["id"], "dev.example.hello")
        second = self.root / "bundled-second"
        bundle(extensions, second, key_id="publisher", seed=seed)
        self.assertEqual((output / "0000.lapis").read_bytes(), (second / "0000.lapis").read_bytes())
        with self.assertRaises(ValueError):
            bundle(extensions, output, key_id="publisher", seed=seed)

    def test_development_bundle_trust_is_scoped_to_each_session(self):
        extensions = self.root / "extensions"
        source = extensions / "example"
        source.mkdir(parents=True)
        (source / "extension.json").write_text(json.dumps(self.metadata), encoding="utf-8")
        (source / "extension.wasm").write_bytes(b"component")
        first, second = self.root / "first", self.root / "second"
        prepare(extensions, first, self.root / "target")
        prepare(extensions, second, self.root / "target")
        registries = []
        for session in (first, second):
            registry = json.loads((session / "hosts/extensions/trusted-keys.json").read_text(encoding="utf-8"))
            public = base64.b64decode(registry["keys"]["lapis-dev-session"])
            registries.append(public)
            with zipfile.ZipFile(session / "bin/bundled-extensions/0000.lapis") as archive:
                manifest = json.loads(archive.read("manifest.json"))
            signature = base64.b64decode(manifest.pop("signature"))
            self.assertTrue(ed25519.verify(public, canonical_bytes(manifest), signature))
        self.assertNotEqual(*registries)
        with self.assertRaises(ValueError):
            prepare(extensions, first, self.root / "target")


if __name__ == "__main__":
    unittest.main()
