//! ハーネスごとの起動方法。ACP の wire 形式ではなく、起動引数だけを持つ。

use std::{env, path::PathBuf};

/// OpenCode の ACP server を stdio で起動するための実行ファイル。
///
/// 実機で確認した起動方法は `opencode acp`（`initialize` / `session/new` /
/// `session/prompt` / `session/update` を stdio の改行区切り JSON-RPC で話す）。
/// `opencode` は npm の shim なので、Windows では `.cmd` を直接起動せず
/// 実体の `.exe` を解決する。
pub fn opencode_spec() -> PathBuf {
    if let Some(path) = env::var_os("LAPIS_OPENCODE_PATH") {
        return PathBuf::from(path);
    }
    if cfg!(windows)
        && let Some(app_data) = env::var_os("APPDATA")
    {
        let candidate = PathBuf::from(app_data)
            .join("npm")
            .join("node_modules")
            .join("@opencode")
            .join("cli")
            .join("bin")
            .join("opencode.exe");
        if candidate.is_file() {
            return candidate;
        }
    }
    PathBuf::from(if cfg!(windows) {
        "opencode.cmd"
    } else {
        "opencode"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opencode_spec_is_never_empty() {
        assert!(!opencode_spec().as_os_str().is_empty());
    }
}
