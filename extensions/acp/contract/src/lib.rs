//! ACP 拡張の wire 境界。
//!
//! ACP 固有の型・実装は `extensions/acp` の host 側（`local-acp`）に閉じ込め、
//! ここでは core と Client が知る必要がある識別子だけを公開する。

/// Task の runner 識別子。実行系の選択はこの文字列だけで表現する。
pub const ACP_RUNNER: &str = "acp-opencode";

/// ACP 経由で起動する最初のハーネス名。
pub const FIRST_HARNESS: &str = "opencode";

/// `TaskStart.runner` が ACP 経由かを判定する。
pub fn is_acp_runner(runner: &str) -> bool {
    runner == ACP_RUNNER
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_acp_runner_matches() {
        assert!(is_acp_runner(ACP_RUNNER));
        assert!(!is_acp_runner("codex-app-server"));
        assert!(!is_acp_runner(""));
    }
}
