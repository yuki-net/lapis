//! launcher の起動経路。実プロセスを起動し、pointer の version と引数転送を確認する。
//!
//! staging の適用（署名検証・展開・prune）は `src/layout.rs` の test が fixture 配布物で
//! 確認する。ここでは「起動時に何があっても本体が起動する」ことだけを固定する。

use std::path::{Path, PathBuf};
use std::process::Command;

/// 起動する本体として複写する OS の shell。`/c` または `-c` で終了 code を返す。
fn shell() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(std::env::var_os("COMSPEC").expect("COMSPEC"))
    } else {
        PathBuf::from("/bin/sh")
    }
}

/// launcher へ渡す、shell に `exit <code>` させる引数。
fn exit_arguments(code: u8) -> Vec<String> {
    if cfg!(windows) {
        vec!["/c".to_owned(), format!("exit {code}")]
    } else {
        vec!["-c".to_owned(), format!("exit {code}")]
    }
}

const MANIFEST: &str = r#"{
  "assets": {
    "windows-x86_64": {
      "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
      "source": "lapis-0.1.0-windows-x86_64.zip"
    }
  },
  "breaking": false,
  "generated_at": "2026-11-12T00:00:00Z",
  "key_id": "launcher-fixture",
  "notes_url": "https://example.invalid/releases/v0.1.0",
  "schema": 1,
  "signature": "AA==",
  "version": "0.1.0"
}
"#;

/// `versions/<version>/` に manifest と OS の shell を `lapis.exe` として置く。
fn install_version(root: &Path, version: &str) {
    let directory = root.join("versions").join(version);
    std::fs::create_dir_all(&directory).expect("version directory");
    std::fs::write(directory.join("manifest.json"), MANIFEST).expect("manifest");
    let executable = directory.join(format!("lapis{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(shell(), &executable).expect("copy shell");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&executable)
            .expect("metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).expect("chmod");
    }
    std::fs::write(
        root.join("current.json"),
        format!("{{\"schema\":1,\"version\":\"{version}\"}}\n"),
    )
    .expect("pointer");
}

fn launcher(root: &Path, arguments: &[String]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_lapis-launcher"))
        .arg("--install-root")
        .arg(root)
        .args(arguments)
        .output()
        .expect("launcher runs")
}

#[test]
fn launches_the_pointer_version_and_forwards_arguments() {
    let root = tempfile::tempdir().expect("temp");
    install_version(root.path(), "0.1.0");
    let output = launcher(root.path(), &exit_arguments(7));
    assert_eq!(
        output.status.code(),
        Some(7),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn corrupt_staging_does_not_block_the_current_version() {
    let root = tempfile::tempdir().expect("temp");
    install_version(root.path(), "0.1.0");
    let staging = root.path().join("staging").join("0.2.0");
    std::fs::create_dir_all(&staging).expect("staging");
    std::fs::write(staging.join("manifest.json"), "not json").expect("broken manifest");
    let output = launcher(root.path(), &exit_arguments(5));
    assert_eq!(output.status.code(), Some(5));
    // 適用していないため pointer は現行版のまま。
    let pointer = std::fs::read_to_string(root.path().join("current.json")).expect("pointer");
    assert!(pointer.contains("0.1.0"));
}

#[test]
fn unknown_staging_version_is_kept_and_current_version_starts() {
    let root = tempfile::tempdir().expect("temp");
    install_version(root.path(), "0.1.0");
    let staging = root.path().join("staging").join("0.2.0");
    std::fs::create_dir_all(&staging).expect("staging");
    let output = launcher(root.path(), &exit_arguments(3));
    assert_eq!(output.status.code(), Some(3));
    assert!(staging.is_dir(), "未適用の staging は残す");
}

#[test]
fn missing_install_reports_an_error_without_launching() {
    let root = tempfile::tempdir().expect("temp");
    let output = launcher(root.path(), &[]);
    assert_ne!(output.status.code(), Some(0));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("install"), "{stderr}");
}
