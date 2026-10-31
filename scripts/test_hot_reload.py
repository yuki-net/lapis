"""Hot Reload launcherがHostを先にビルドし、dxの出力削除に依存しないことを検証する。"""

import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]


@unittest.skipUnless(sys.platform == "win32", "Windows専用launcher")
class HotReloadTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.directory = tempfile.TemporaryDirectory(prefix="lapis hot reload ")
        cls.addClassCleanup(cls.directory.cleanup)
        cls.root = Path(cls.directory.name)
        launcher = cls.root / "scripts" / "hot-reload"
        shutil.copytree(ROOT / "scripts" / "hot-reload", launcher)
        subprocess.run([
            "cargo", "build", "--locked", "--manifest-path", str(launcher / "Cargo.toml"),
            "--target-dir", str(cls.root / "launcher-target"),
        ], check=True, capture_output=True)
        cls.launcher = cls.root / "launcher-target/debug/lapis-hot-reload-launcher.exe"
        source = cls.root / "fixture.rs"
        source.write_text(r'''
use std::{env, fs, path::Path, process, thread, time::Duration};
fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if env::current_exe().unwrap().file_stem().unwrap() == "cargo-fixture" {
        fs::write("cargo-called", "yes").unwrap();
        assert_eq!(args, ["build", "--locked", "--target-dir", "target", "--target",
            "x86_64-pc-windows-msvc", "--profile", "hot-reload", "--package", "lapis-host",
            "--bin", "lapis-host"]);
        if env::var("LAPIS_TEST_SCENARIO").unwrap() == "build-failure" { process::exit(23); }
        let host = Path::new("target/x86_64-pc-windows-msvc/hot-reload/lapis-host.exe");
        fs::create_dir_all(host.parent().unwrap()).unwrap();
        fs::write(host, "fresh-host").unwrap();
        return;
    }
    assert_eq!(&args[..5], ["serve", "--platform", "windows", "--target", "x86_64-pc-windows-msvc"]);
    // dx 0.7.10と同様に、app出力を削除してから起動引数を解釈する。
    let app = Path::new("target/dx/lapis/debug/windows/app");
    fs::create_dir_all(app).unwrap();
    fs::write(app.join("lapis-host.exe"), "stale-copy").unwrap();
    fs::remove_dir_all(app).unwrap();
    let app_args = args.iter().find_map(|arg| arg.strip_prefix("--args=")).unwrap();
    let app_args: Vec<_> = app_args.split_whitespace().collect();
    let index = app_args.iter().position(|arg| *arg == "--local-host-executable").unwrap();
    assert_eq!(fs::read_to_string(app_args[index + 1]).unwrap(), "fresh-host");
    fs::write("dx-called", "yes").unwrap();
    if env::var("LAPIS_TEST_SCENARIO").unwrap() == "hold" {
        while !Path::new("release-dx").exists() { thread::sleep(Duration::from_millis(10)); }
    }
    process::exit(17);
}
''', encoding="utf-8")
        cls.cargo = cls.root / "cargo-fixture.exe"
        subprocess.run(["rustc", str(source), "-o", str(cls.cargo)], check=True, capture_output=True)
        cls.dx = cls.root / "target/dx-fixed0710/bin/dx.exe"
        cls.dx.parent.mkdir(parents=True)

    def setUp(self):
        shutil.copyfile(self.cargo, self.dx)
        for name in ("cargo-called", "dx-called", "release-dx"):
            (self.root / name).unlink(missing_ok=True)
        self.env = dict(os.environ, CARGO=str(self.cargo), LAPIS_TEST_SCENARIO="success")

    def run_launcher(self):
        # 起動元cwdが異なってもlauncherがworktreeルートを使う。
        return subprocess.run([str(self.launcher)], cwd=self.root.parent, env=self.env,
                              capture_output=True, timeout=20)

    def test_clean_and_repeated_launch_build_host_before_dx_clears_app_output(self):
        host = self.root / "target/x86_64-pc-windows-msvc/hot-reload/lapis-host.exe"
        host.unlink(missing_ok=True)
        for _ in range(2):
            result = self.run_launcher()
            self.assertEqual(result.returncode, 17, result.stderr.decode(errors="replace"))
            self.assertTrue((self.root / "dx-called").exists())
            self.assertEqual(host.read_text(), "fresh-host")
            host.write_text("old-host")

    def test_host_build_failure_does_not_launch_dx_or_use_stale_host(self):
        self.env["LAPIS_TEST_SCENARIO"] = "build-failure"
        result = self.run_launcher()
        self.assertEqual(result.returncode, 23)
        self.assertFalse((self.root / "dx-called").exists())

    def test_missing_dx_does_not_build_host(self):
        self.dx.unlink()
        result = self.run_launcher()
        self.assertEqual(result.returncode, 1)
        self.assertFalse((self.root / "cargo-called").exists())

    def test_cargo_spawn_failure_does_not_launch_dx(self):
        self.env["CARGO"] = str(self.root / "missing-cargo.exe")
        result = self.run_launcher()
        self.assertEqual(result.returncode, 1)
        self.assertFalse((self.root / "dx-called").exists())

    def test_second_launcher_cannot_rebuild_host_while_first_is_running(self):
        import time
        self.env["LAPIS_TEST_SCENARIO"] = "hold"
        first = subprocess.Popen([str(self.launcher)], cwd=self.root, env=self.env,
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        try:
            deadline = time.monotonic() + 15
            while not (self.root / "dx-called").exists():
                if first.poll() is not None or time.monotonic() > deadline:
                    self.fail("fixture dx did not start")
                time.sleep(0.02)
            (self.root / "cargo-called").unlink()
            second = self.run_launcher()
            self.assertEqual(second.returncode, 1)
            self.assertFalse((self.root / "cargo-called").exists())
        finally:
            (self.root / "release-dx").touch()
            first.communicate(timeout=20)
        self.assertEqual(first.returncode, 17)


if __name__ == "__main__":
    unittest.main()
