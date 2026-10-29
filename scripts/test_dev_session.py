"""WindowsのCargo停止と開発sessionのprocess分離を実processで検証する。"""

import ctypes
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
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
            (root / ".cargo" / "config.toml").write_text('[alias]\nide-build = "version"\n', encoding="utf-8")
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
            (root / ".cargo" / "config.toml").write_text('[alias]\nide-build = "version"\n', encoding="utf-8")
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
            for name in ("lapis.exe", "lapis-host.exe"):
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
                existing_client.terminate()
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
                standalone.terminate()
                standalone.wait(timeout=10)
                self.assertEqual(kernel.WaitForSingleObject(standalone_handle, 10000), 0)
                self.assertIsNone(remote.poll())
                self.assertNotEqual(first_hosts, second_hosts)
                # RustRoverはlauncherへCtrl+Cを送り、Cargoは正常終了し得る。
                # launcherを直接終了してsession Jobの動作を検証する。
                launcher_pid = int(first_hosts.parent.name.split("-", 1)[0])
                launcher_handle = kernel.OpenProcess(0x00101001, False, launcher_pid)
                self.assertTrue(launcher_handle)
                try:
                    image = ctypes.create_unicode_buffer(32768)
                    size = ctypes.c_uint32(len(image))
                    self.assertTrue(kernel.QueryFullProcessImageNameW(launcher_handle, 0, image, ctypes.byref(size)))
                    self.assertEqual(Path(image.value).resolve(), (root / "launcher-target" / "debug" / "lapis-dev-session.exe").resolve())
                    self.assertTrue(kernel.TerminateProcess(launcher_handle, 1))
                    self.assertEqual(kernel.WaitForSingleObject(launcher_handle, 10000), 0)
                finally:
                    kernel.CloseHandle(launcher_handle)
                first.wait(timeout=10)
                self.assertIn(f"Desktop session: {first_hosts.parent.name}".encode(), first.stderr.read())
                self.assertEqual(kernel.WaitForSingleObject(first_handle, 10000), 0)
                self.assertEqual(kernel.WaitForSingleObject(second_handle, 0), 0x102)
                self.assertIsNone(remote.poll())
                (second_hosts / "finish").touch()
                self.assertEqual(second.wait(timeout=10), 0)
                self.assertEqual(kernel.WaitForSingleObject(second_handle, 0), 0x102)
            finally:
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


if __name__ == "__main__":
    unittest.main()
