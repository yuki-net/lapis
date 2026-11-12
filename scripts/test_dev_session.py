"""WindowsのCargo停止と開発sessionのprocess分離を実processで検証する。"""

import ctypes
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import unittest


ROOT = Path(__file__).resolve().parents[1]


@unittest.skipUnless(sys.platform == "win32", "WindowsのCargo Job Objectを検証")
class DevSessionProcessTests(unittest.TestCase):
    def test_startup_failures_report_error_without_killing_launcher(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            launcher = root / "scripts" / "dev-session"
            launcher.mkdir(parents=True)
            shutil.copytree(ROOT / "scripts" / "dev-session" / "src", launcher / "src")
            for name in ("Cargo.toml", "Cargo.lock"):
                shutil.copyfile(ROOT / "scripts" / "dev-session" / name, launcher / name)
            (root / ".cargo").mkdir()
            (root / ".cargo" / "config.toml").write_text('[alias]\nide-build = "version"\nhost-build = "version"\n', encoding="utf-8")
            build_dir = root / "target" / "dev-build" / "debug"
            build_dir.mkdir(parents=True)
            source = root / "fixture.rs"
            source.write_text('fn main() {}', encoding="utf-8")
            valid = root / "valid.exe"
            subprocess.run(["rustc", str(source), "-o", str(valid)], check=True, capture_output=True)
            subprocess.run(["cargo", "build", "--locked", "--manifest-path", str(launcher / "Cargo.toml"), "--target-dir", str(root / "launcher-target"), "--quiet"], cwd=root, check=True, capture_output=True)
            executable = root / "launcher-target" / "debug" / "lapis-dev-session.exe"
            sessions = root / "target" / "dev-sessions"
            for scenario in ("session", "missing_host", "copy", "spawn"):
                with self.subTest(scenario=scenario):
                    shutil.copyfile(valid, build_dir / "lapis.exe")
                    shutil.copyfile(valid, build_dir / "lapis-host.exe")
                    shutil.copyfile(valid, build_dir / "lapis-web-bridge.exe")
                    if scenario == "session":
                        sessions.write_text("not a directory")
                    elif scenario == "missing_host":
                        (build_dir / "lapis-host.exe").unlink()
                    elif scenario == "copy":
                        (build_dir / "lapis-host.exe").unlink()
                        (build_dir / "lapis-host.exe").mkdir()
                    elif scenario == "spawn":
                        (build_dir / "lapis.exe").write_bytes(b"invalid executable")
                    result = subprocess.run([str(executable)], cwd=root, capture_output=True, timeout=20)
                    self.assertEqual(result.returncode, 1, result.stderr.decode("utf-8", errors="replace"))
                    self.assertIn("開発sessionを開始できません", result.stderr.decode("utf-8", errors="replace"))
                    if scenario == "session":
                        sessions.unlink()
                    if scenario == "copy":
                        (build_dir / "lapis-host.exe").rmdir()

    def test_normal_close_and_stop_do_not_cross_session_boundaries(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            launcher = root / "scripts" / "dev-session"
            launcher.mkdir(parents=True)
            shutil.copytree(ROOT / "scripts" / "dev-session" / "src", launcher / "src")
            for filename in ("Cargo.toml", "Cargo.lock"):
                shutil.copyfile(ROOT / "scripts" / "dev-session" / filename, launcher / filename)
            (root / ".cargo").mkdir()
            (root / ".cargo" / "config.toml").write_text('[alias]\nide-build = "version"\nhost-build = "version"\n', encoding="utf-8")
            binary_dir = root / "target" / "dev-build" / "debug"
            binary_dir.mkdir(parents=True)
            source = root / "fixture.rs"
            source.write_text(r'''
use std::{env, fs, path::PathBuf, process::{Command, Stdio}, thread, time::Duration};
fn main() {
    let args: Vec<_> = env::args_os().collect();
    if args.iter().any(|v| v == "--fixture-host") {
        thread::sleep(Duration::from_secs(120));
        return;
    }
    if env::current_exe().unwrap().file_name().unwrap() == "lapis-host.exe" {
        let certificate = PathBuf::from(args[args.iter().position(|v| v == "--certificate").unwrap()+1].clone());
        fs::write(certificate.parent().unwrap().join("standalone.pid"), std::process::id().to_string()).unwrap();
        thread::sleep(Duration::from_secs(120));
        return;
    }
    let hosts = PathBuf::from(args[args.iter().position(|v| v == "--local-host-directory").unwrap()+1].clone());
    if hosts.join("existing").exists() {
        fs::write(hosts.join("connected"), b"yes").unwrap();
    } else {
        let host = Command::new(env::current_exe().unwrap().with_file_name("lapis-host.exe"))
            .arg("--fixture-host").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        fs::write(hosts.join("host.pid"), host.id().to_string()).unwrap();
    }
    while !hosts.join("finish").exists() { thread::sleep(Duration::from_millis(50)); }
}
''', encoding="utf-8")
            for name in ("lapis.exe", "lapis-host.exe", "lapis-web-bridge.exe"):
                subprocess.run(["rustc", str(source), "-o", str(binary_dir / name)], check=True, capture_output=True)
            # 起動待ちの期限には初回buildや依存取得を含めない。
            subprocess.run(
                ["cargo", "build", "--locked", "--manifest-path", str(launcher / "Cargo.toml"), "--target-dir", str(root / "launcher-target"), "--quiet"],
                cwd=root, check=True, capture_output=True,
            )

            kernel = ctypes.WinDLL("kernel32", use_last_error=True)
            kernel.OpenProcess.argtypes = [ctypes.c_uint32, ctypes.c_int, ctypes.c_uint32]
            kernel.OpenProcess.restype = ctypes.c_void_p
            kernel.QueryFullProcessImageNameW.argtypes = [ctypes.c_void_p, ctypes.c_uint32, ctypes.c_wchar_p, ctypes.POINTER(ctypes.c_uint32)]
            kernel.QueryFullProcessImageNameW.restype = ctypes.c_int
            kernel.WaitForSingleObject.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
            kernel.WaitForSingleObject.restype = ctypes.c_uint32
            kernel.TerminateProcess.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
            kernel.CloseHandle.argtypes = [ctypes.c_void_p]
            handles = []
            launchers = []
            launcher_handles = {}

            def launcher_handle_for(session):
                if session in launcher_handles:
                    return launcher_handles[session]
                pid = int(session.name.split("-", 1)[0])
                handle = kernel.OpenProcess(0x00101001, False, pid)
                self.assertTrue(handle)
                image = ctypes.create_unicode_buffer(32768)
                size = ctypes.c_uint32(len(image))
                if not kernel.QueryFullProcessImageNameW(handle, 0, image, ctypes.byref(size)) or Path(image.value).resolve() != (root / "launcher-target" / "debug" / "lapis-dev-session.exe").resolve():
                    kernel.CloseHandle(handle)
                    self.fail("unexpected launcher process identity")
                launcher_handles[session] = handle
                return handle

            def stop_launcher(session):
                handle = launcher_handle_for(session)
                self.assertTrue(kernel.TerminateProcess(handle, 1))
                self.assertEqual(kernel.WaitForSingleObject(handle, 10000), 0)

            other_binary = root / "other-worktree" / "target" / "debug" / "lapis-host.exe"
            other_binary.parent.mkdir(parents=True)
            shutil.copyfile(binary_dir / "lapis-host.exe", other_binary)
            remote = subprocess.Popen(
                [str(other_binary), "--fixture-host"],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                creationflags=subprocess.CREATE_NO_WINDOW,
            )

            def start():
                before = set((root / "target" / "dev-sessions").glob("*")) if (root / "target" / "dev-sessions").exists() else set()
                process = subprocess.Popen(
                    ["cargo", "run", "--locked", "--manifest-path", str(launcher / "Cargo.toml"), "--target-dir", str(root / "launcher-target"), "--quiet"],
                    cwd=root, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                    creationflags=subprocess.CREATE_NO_WINDOW,
                )
                launchers.append(process)
                deadline = time.monotonic() + 40
                while time.monotonic() < deadline:
                    sessions = set((root / "target" / "dev-sessions").glob("*")) - before
                    if len(sessions) == 1:
                        hosts = sessions.pop() / "hosts"
                        launcher_handle_for(hosts.parent)
                        try:
                            pid = int((hosts / "host.pid").read_text())
                            handle = kernel.OpenProcess(0x00101001, False, pid)
                            if handle:
                                image = ctypes.create_unicode_buffer(32768)
                                size = ctypes.c_uint32(len(image))
                                if not kernel.QueryFullProcessImageNameW(handle, 0, image, ctypes.byref(size)) or Path(image.value).resolve() != (hosts.parent / "bin" / "lapis-host.exe").resolve():
                                    kernel.CloseHandle(handle)
                                    self.fail("unexpected host process identity")
                                handles.append(handle)
                                return process, hosts, handle
                        except (OSError, ValueError):
                            pass
                    if process.poll() is not None:
                        self.fail(f"launcher exited: {process.returncode}")
                    time.sleep(0.05)
                self.fail("session host did not start")

            try:
                first, first_hosts, first_handle = start()
                second, second_hosts, second_handle = start()
                existing_hosts = root / "existing-hosts"
                existing_hosts.mkdir()
                (existing_hosts / "existing").touch()
                before_existing = set((root / "target" / "dev-sessions").glob("*"))
                existing_client = subprocess.Popen(
                    ["cargo", "run", "--locked", "--manifest-path", str(launcher / "Cargo.toml"), "--target-dir", str(root / "launcher-target"), "--quiet", "--", "--local-host-directory", str(existing_hosts)],
                    cwd=root, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                    creationflags=subprocess.CREATE_NO_WINDOW,
                )
                launchers.append(existing_client)
                deadline = time.monotonic() + 40
                while not (existing_hosts / "connected").exists() and time.monotonic() < deadline:
                    if existing_client.poll() is not None:
                        self.fail("existing-host client exited before connecting")
                    time.sleep(0.05)
                self.assertTrue((existing_hosts / "connected").exists())
                existing_sessions = set((root / "target" / "dev-sessions").glob("*")) - before_existing
                self.assertEqual(len(existing_sessions), 1)
                stop_launcher(existing_sessions.pop())
                existing_client.wait(timeout=10)
                self.assertIsNone(remote.poll())
                before = set((root / "target" / "dev-sessions").glob("*"))
                standalone = subprocess.Popen(
                    ["cargo", "run", "--locked", "--manifest-path", str(launcher / "Cargo.toml"), "--target-dir", str(root / "launcher-target"), "--quiet", "--", "--host-only"],
                    cwd=root, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                    creationflags=subprocess.CREATE_NO_WINDOW,
                )
                launchers.append(standalone)
                deadline = time.monotonic() + 40
                standalone_handle = None
                while time.monotonic() < deadline:
                    sessions = set((root / "target" / "dev-sessions").glob("*")) - before
                    if len(sessions) == 1:
                        session = sessions.pop()
                        launcher_handle_for(session)
                        try:
                            pid = int((session / "standalone.pid").read_text())
                            standalone_handle = kernel.OpenProcess(0x00101001, False, pid)
                            if standalone_handle:
                                image = ctypes.create_unicode_buffer(32768)
                                size = ctypes.c_uint32(len(image))
                                if not kernel.QueryFullProcessImageNameW(standalone_handle, 0, image, ctypes.byref(size)) or Path(image.value).resolve() != (session / "bin" / "lapis-host.exe").resolve():
                                    kernel.CloseHandle(standalone_handle)
                                    self.fail("unexpected standalone host identity")
                                handles.append(standalone_handle)
                                break
                        except (OSError, ValueError):
                            pass
                    if standalone.poll() is not None:
                        self.fail("standalone host exited")
                    time.sleep(0.05)
                self.assertTrue(standalone_handle)
                stop_launcher(session)
                standalone.wait(timeout=10)
                self.assertEqual(kernel.WaitForSingleObject(standalone_handle, 10000), 0)
                self.assertIsNone(remote.poll())
                self.assertEqual(kernel.WaitForSingleObject(first_handle, 0), 0x102)
                self.assertEqual(kernel.WaitForSingleObject(second_handle, 0), 0x102)
                self.assertNotEqual(first_hosts, second_hosts)
                # RustRoverはlauncherへCtrl+Cを送り、Cargoは正常終了し得る。
                # launcherを直接終了してsession Jobの動作を検証する。
                stop_launcher(first_hosts.parent)
                first.wait(timeout=10)
                self.assertIn(f"Desktop session: {first_hosts.parent.name}".encode(), first.stderr.read())
                self.assertEqual(kernel.WaitForSingleObject(first_handle, 10000), 0)
                self.assertEqual(kernel.WaitForSingleObject(second_handle, 0), 0x102)
                self.assertIsNone(remote.poll())
                (second_hosts / "finish").touch()
                self.assertEqual(second.wait(timeout=10), 0)
                self.assertEqual(kernel.WaitForSingleObject(second_handle, 0), 0x102)
            finally:
                # 起動待ちの途中で失敗しても、このfixtureのlauncherだけ回収する。
                for session in (root / "target" / "dev-sessions").glob("*"):
                    if session in launcher_handles:
                        continue
                    try:
                        pid = int(session.name.split("-", 1)[0])
                    except ValueError:
                        continue
                    handle = kernel.OpenProcess(0x00101001, False, pid)
                    if handle:
                        image = ctypes.create_unicode_buffer(32768)
                        size = ctypes.c_uint32(len(image))
                        if kernel.QueryFullProcessImageNameW(handle, 0, image, ctypes.byref(size)) and Path(image.value).resolve() == (root / "launcher-target" / "debug" / "lapis-dev-session.exe").resolve():
                            launcher_handles[session] = handle
                        else:
                            kernel.CloseHandle(handle)
                for handle in launcher_handles.values():
                    if kernel.WaitForSingleObject(handle, 0) == 0x102:
                        kernel.TerminateProcess(handle, 1)
                        kernel.WaitForSingleObject(handle, 10000)
                    kernel.CloseHandle(handle)
                for process in launchers:
                    if process.poll() is None:
                        process.terminate()
                        process.wait(timeout=10)
                    if process.stderr is not None:
                        process.stderr.close()
                for handle in handles:
                    if kernel.WaitForSingleObject(handle, 0) == 0x102:
                        kernel.TerminateProcess(handle, 1)
                        kernel.WaitForSingleObject(handle, 10000)
                    kernel.CloseHandle(handle)
                if remote.poll() is None:
                    remote.terminate()
                    remote.wait(timeout=10)


@unittest.skipUnless(sys.platform == "win32", "Windows専用launcher")
class HotReloadTests(unittest.TestCase):
    """Hot Reload launcherがHostを先にビルドし、dxの出力削除に依存しないことを検証する。"""

    @classmethod
    def setUpClass(cls):
        cls.directory = tempfile.TemporaryDirectory(prefix="lapis hot reload ")
        cls.addClassCleanup(cls.directory.cleanup)
        cls.root = Path(cls.directory.name)
        launcher = cls.root / "scripts" / "dev-session"
        launcher.mkdir(parents=True)
        shutil.copytree(ROOT / "scripts" / "dev-session" / "src", launcher / "src")
        for name in ("Cargo.toml", "Cargo.lock"):
            shutil.copyfile(ROOT / "scripts" / "dev-session" / name, launcher / name)
        subprocess.run([
            "cargo", "build", "--locked", "--manifest-path", str(launcher / "Cargo.toml"),
            "--target-dir", str(cls.root / "launcher-target"),
        ], check=True, capture_output=True)
        cls.launcher = cls.root / "launcher-target/debug/lapis-dev-session.exe"
        source = cls.root / "fixture.rs"
        source.write_text(r'''
use std::{env, fs, path::Path, process, thread, time::Duration};
fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if env::current_exe().unwrap().file_stem().unwrap() == "cargo-fixture" {
        fs::write("cargo-called", "yes").unwrap();
        assert_eq!(args, ["build", "--locked", "--target-dir", "target", "--target",
            "x86_64-pc-windows-msvc", "--profile", "hot-reload", "--package", "host",
            "--package", "web-bridge", "--bins"]);
        if env::var("LAPIS_TEST_SCENARIO").unwrap() == "build-failure" { process::exit(23); }
        let host = Path::new("target/x86_64-pc-windows-msvc/hot-reload/lapis-host.exe");
        fs::create_dir_all(host.parent().unwrap()).unwrap();
        fs::write(host, "fresh-host").unwrap();
        fs::write(host.with_file_name("lapis-web-bridge.exe"), "fresh-bridge").unwrap();
        return;
    }
    assert_eq!(&args[..5], ["serve", "--platform", "windows", "--target", "x86_64-pc-windows-msvc"]);
    // dx 0.7.10と同様に、app出力を削除してから起動引数を解釈する。
    let app = Path::new("target/dx/lapis/debug/windows/app");
    fs::create_dir_all(app).unwrap();
    fs::write(app.join("lapis-host.exe"), "stale-copy").unwrap();
    fs::remove_dir_all(app).unwrap();
    let app_args = args.iter().find_map(|arg| arg.strip_prefix("--args=")).unwrap();
    // dxは空白で引数を分割するため、launcherが渡すパスはworktreeルートからの相対でなければならない。
    let app_args: Vec<_> = app_args.split_whitespace().collect();
    let index = app_args.iter().position(|arg| *arg == "--local-host-executable").unwrap();
    assert_eq!(fs::read_to_string(app_args[index + 1]).unwrap(), "fresh-host");
    assert_eq!(fs::read_to_string(Path::new(app_args[index + 1]).with_file_name("lapis-web-bridge.exe")).unwrap(), "fresh-bridge");
    fs::write("dx-app-args", app_args.join("\n")).unwrap();
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
        for name in ("cargo-called", "dx-called", "dx-app-args", "release-dx"):
            (self.root / name).unlink(missing_ok=True)
        self.env = dict(os.environ, CARGO=str(self.cargo), LAPIS_TEST_SCENARIO="success")

    def run_launcher(self):
        # 起動元cwdが異なってもlauncherがworktreeルートを使う。
        return subprocess.run(
            [str(self.launcher), "--hot-reload"], cwd=self.root.parent, env=self.env,
            capture_output=True, timeout=20,
        )

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
        self.assertEqual(result.returncode, 23, result.stderr.decode(errors="replace"))
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

    def test_hot_reload_waits_for_host_invitation_before_dx(self):
        """Desktopと同じく、Hostの招待が現れるまでdxを起動しないことを検証する。"""
        # Desktopと同じく、Hostを別のRunで起動している場合は招待が現れるまで待つ。
        invitation = self.root / "target/dev-desktop/desktop.json"
        invitation.parent.mkdir(parents=True, exist_ok=True)
        invitation.unlink(missing_ok=True)
        process = subprocess.Popen(
            [str(self.launcher), "--hot-reload", "--host", "target/dev-desktop/desktop.json"],
            cwd=self.root.parent, env=self.env,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        try:
            time.sleep(0.5)
            self.assertFalse((self.root / "dx-called").exists())
            invitation.write_text("{}", encoding="utf-8")
            process.communicate(timeout=20)
        finally:
            if process.poll() is None:
                process.kill()
                process.communicate()
        self.assertEqual(process.returncode, 17)

    def app_arguments(self):
        return (self.root / "dx-app-args").read_text(encoding="utf-8").splitlines()

    def test_hot_reload_opens_the_local_workspace_without_host(self):
        result = self.run_launcher()
        self.assertEqual(result.returncode, 17, result.stderr.decode(errors="replace"))
        self.assertIn("--workspace", self.app_arguments())

    def test_hot_reload_with_host_leaves_the_workspace_to_the_host(self):
        invitation = self.root / "target/dev-desktop/desktop.json"
        invitation.parent.mkdir(parents=True, exist_ok=True)
        invitation.write_text("{}", encoding="utf-8")
        result = subprocess.run(
            [str(self.launcher), "--hot-reload", "--host", "target/dev-desktop/desktop.json"],
            cwd=self.root.parent, env=self.env, capture_output=True, timeout=20,
        )
        self.assertEqual(result.returncode, 17, result.stderr.decode(errors="replace"))
        self.assertNotIn("--workspace", self.app_arguments())

    def test_argument_with_whitespace_is_rejected_before_building(self):
        # dx は --args= を空白で分割するため、空白入りの引数は渡す前に拒否する。
        result = subprocess.run(
            [str(self.launcher), "--hot-reload", "C:\\my dir"], cwd=self.root.parent,
            env=self.env, capture_output=True, timeout=20,
        )
        self.assertEqual(result.returncode, 1, result.stderr.decode(errors="replace"))
        self.assertIn("空白", result.stderr.decode("utf-8", errors="replace"))
        self.assertFalse((self.root / "cargo-called").exists())
        self.assertFalse((self.root / "dx-called").exists())

    def test_second_launcher_cannot_rebuild_host_while_first_is_running(self):
        self.env["LAPIS_TEST_SCENARIO"] = "hold"
        first = subprocess.Popen(
            [str(self.launcher), "--hot-reload"], cwd=self.root, env=self.env,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
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


@unittest.skipUnless(sys.platform == "win32", "Windows専用launcher")
class HostFileWaitTests(unittest.TestCase):
    """Hostを別のRunで起動しても、Desktopの起動が招待の出現を待つことを検証する。"""

    @classmethod
    def setUpClass(cls):
        cls.directory = tempfile.TemporaryDirectory(prefix="lapis host file wait ")
        cls.addClassCleanup(cls.directory.cleanup)
        cls.root = Path(cls.directory.name)
        launcher = cls.root / "scripts" / "dev-session"
        launcher.mkdir(parents=True)
        shutil.copytree(ROOT / "scripts" / "dev-session" / "src", launcher / "src")
        for name in ("Cargo.toml", "Cargo.lock"):
            shutil.copyfile(ROOT / "scripts" / "dev-session" / name, launcher / name)
        (cls.root / ".cargo").mkdir()
        (cls.root / ".cargo" / "config.toml").write_text('[alias]\nide-build = "version"\n', encoding="utf-8")
        binary_dir = cls.root / "target" / "dev-build" / "debug"
        binary_dir.mkdir(parents=True)
        source = cls.root / "fixture.rs"
        source.write_text(r'''
use std::{env, fs};
fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let host = args.iter().position(|arg| arg == "--host").map(|index| args[index + 1].clone());
    fs::write("desktop-started", host.unwrap_or_default()).unwrap();
}
''', encoding="utf-8")
        for name in ("lapis.exe", "lapis-host.exe", "lapis-web-bridge.exe"):
            subprocess.run(["rustc", str(source), "-o", str(binary_dir / name)], check=True, capture_output=True)
        subprocess.run([
            "cargo", "build", "--locked", "--manifest-path", str(launcher / "Cargo.toml"),
            "--target-dir", str(cls.root / "launcher-target"), "--quiet",
        ], cwd=cls.root, check=True, capture_output=True)
        cls.launcher = cls.root / "launcher-target" / "debug" / "lapis-dev-session.exe"

    def setUp(self):
        self.hosts = self.root / "dev"
        self.hosts.mkdir(exist_ok=True)
        for path in self.hosts.iterdir():
            path.unlink()
        (self.root / "desktop-started").unlink(missing_ok=True)
        self.env = dict(os.environ)
        self.env.pop("LAPIS_HOST_FILE", None)

    def test_desktop_starts_after_host_file_appears(self):
        threading.Timer(1.0, (self.hosts / "desktop.json").write_text, args=("{}",)).start()
        started = time.monotonic()
        result = subprocess.run(
            [str(self.launcher), "--host", "dev/desktop.json"], cwd=self.root,
            env=self.env, capture_output=True, timeout=30,
        )
        waited = time.monotonic() - started
        stderr = result.stderr.decode("utf-8", errors="replace")
        self.assertEqual(result.returncode, 0, stderr)
        self.assertIn("Host の接続情報を待っています", stderr)
        self.assertGreater(waited, 0.8)
        self.assertEqual((self.root / "desktop-started").read_text(), "dev/desktop.json")

    def test_existing_host_file_starts_without_waiting(self):
        (self.hosts / "desktop.host").write_text("{}")
        result = subprocess.run(
            [str(self.launcher), "--host", "dev/desktop.host"], cwd=self.root,
            env=self.env, capture_output=True, timeout=30,
        )
        stderr = result.stderr.decode("utf-8", errors="replace")
        self.assertEqual(result.returncode, 0, stderr)
        self.assertNotIn("Host の接続情報を待っています", stderr)
        self.assertEqual((self.root / "desktop-started").read_text(), "dev/desktop.host")


@unittest.skipUnless(sys.platform == "win32", "Windows専用launcher")
class HostOnlyBuildTests(unittest.TestCase):
    """`--host-only` が Desktop をビルドせず、Host だけをビルドすることを検証する。"""

    @classmethod
    def setUpClass(cls):
        cls.directory = tempfile.TemporaryDirectory(prefix="lapis host only ")
        cls.addClassCleanup(cls.directory.cleanup)
        cls.root = Path(cls.directory.name)
        launcher = cls.root / "scripts" / "dev-session"
        launcher.mkdir(parents=True)
        shutil.copytree(ROOT / "scripts" / "dev-session" / "src", launcher / "src")
        for name in ("Cargo.toml", "Cargo.lock"):
            shutil.copyfile(ROOT / "scripts" / "dev-session" / name, launcher / name)
        # ide-build だけを失敗させ、--host-only が Desktop をビルドしないことを示す。
        (cls.root / ".cargo").mkdir()
        (cls.root / ".cargo" / "config.toml").write_text(
            '[alias]\nide-build = "no-such-command-for-ide-build"\nhost-build = "version"\n',
            encoding="utf-8",
        )
        binary_dir = cls.root / "target" / "dev-build" / "debug"
        binary_dir.mkdir(parents=True)
        source = cls.root / "fixture.rs"
        source.write_text("fn main() {}\n", encoding="utf-8")
        for name in ("lapis.exe", "lapis-host.exe"):
            subprocess.run(["rustc", str(source), "-o", str(binary_dir / name)], check=True, capture_output=True)
        subprocess.run([
            "cargo", "build", "--locked", "--manifest-path", str(launcher / "Cargo.toml"),
            "--target-dir", str(cls.root / "launcher-target"), "--quiet",
        ], cwd=cls.root, check=True, capture_output=True)
        cls.launcher = cls.root / "launcher-target" / "debug" / "lapis-dev-session.exe"

    def test_host_only_builds_without_desktop(self):
        result = subprocess.run(
            [str(self.launcher), "--host-only"], cwd=self.root,
            capture_output=True, timeout=30,
        )
        stderr = result.stderr.decode("utf-8", errors="replace")
        self.assertEqual(result.returncode, 0, stderr)
        self.assertIn("Host session:", stderr)

    def test_default_build_keeps_building_desktop(self):
        result = subprocess.run(
            [str(self.launcher)], cwd=self.root, capture_output=True, timeout=30,
        )
        self.assertNotEqual(result.returncode, 0)


if __name__ == "__main__":
    unittest.main()
