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
    let status = std::process::Command::new(dx)
        .current_dir(root)
        .args([
            "serve",
            "--platform",
            "windows",
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
            "--args=--state-directory target/dev-state --workspace .",
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
