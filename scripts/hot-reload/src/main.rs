#[cfg(windows)]
fn main() {
    use std::os::windows::fs::OpenOptionsExt;

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("worktree root");
    let dx = root.join("target/dx-fixed0710/bin/dx.exe");
    if !dx.is_file() {
        eprintln!(
            "修正版 dx がありません。apps/desktop/ui/experiments/hot-reload/README.md の手順で準備してください。"
        );
        std::process::exit(1);
    }
    let lock = root.join("target/hot-reload.lock");
    let _lock = match std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(lock)
    {
        Ok(lock) => lock,
        Err(error) => {
            eprintln!("Hot Reload を開始できません。既に起動中か確認してください: {error}");
            std::process::exit(1);
        }
    };
    // dx は Desktop だけをビルドし、起動時に app 出力先を削除する。
    // Host は Cargo の安定した出力先に用意し、Composition Root へ明示する。
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let build = std::process::Command::new(cargo)
        .current_dir(root)
        .args([
            "build",
            "--locked",
            "--target-dir",
            "target",
            "--target",
            "x86_64-pc-windows-msvc",
            "--profile",
            "hot-reload",
            "--package",
            "lapis-host",
            "--bin",
            "lapis-host",
        ])
        .status();
    match build {
        Ok(status) if status.success() => {}
        Ok(status) => {
            eprintln!("Local Host のビルドに失敗したため、Hot Reload を開始しません。");
            std::process::exit(status.code().unwrap_or(1));
        }
        Err(error) => {
            eprintln!("Local Host のビルドを開始できません: {error}");
            std::process::exit(1);
        }
    }
    let status = std::process::Command::new(dx)
        .current_dir(root)
        .args([
            "serve",
            "--platform",
            "windows",
            "--target",
            "x86_64-pc-windows-msvc",
            "--hot-patch",
            "--interactive",
            "false",
            "--open",
            "false",
            "--package",
            "lapis",
            "--features",
            "hot-reload",
            "--profile",
            "hot-reload",
            "--args=--state-directory target/dev-state --workspace . --local-host-executable target/x86_64-pc-windows-msvc/hot-reload/lapis-host.exe",
        ])
        .status();
    match status {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(error) => {
            eprintln!("修正版 dx を起動できません: {error}");
            std::process::exit(1);
        }
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("この Hot Reload 起動設定は Windows 用です。");
    std::process::exit(1);
}
