//! Web Client が Host へ要求する capability セット。
//!
//! 操作名は `client_api::capability` の const を正本とし、本番 Web と検証用
//! WebSession は同じ定義を共有する。文字列リテラルを複写しない。

use client_api::capability::{
    DOCUMENTS_READ, DOCUMENTS_WRITE, FILES_READ, FILES_WRITE, TERMINAL_CONTROL, TERMINAL_START,
    WORKBENCH_WRITE, WORKSPACE_SYNC, WORKSPACES, WORKSPACES_CONNECT, WORKSPACES_SWITCH,
};

/// Web Client が Host へ要求する capability。
pub const CAPABILITIES: &[&str] = &[
    WORKSPACES,
    WORKSPACES_CONNECT,
    WORKSPACES_SWITCH,
    WORKSPACE_SYNC,
    WORKBENCH_WRITE,
    DOCUMENTS_READ,
    DOCUMENTS_WRITE,
    FILES_READ,
    FILES_WRITE,
    TERMINAL_START,
    TERMINAL_CONTROL,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_are_unique_and_valid() {
        let mut seen = std::collections::HashSet::new();
        for capability in CAPABILITIES {
            assert!(
                seen.insert(*capability),
                "capability が重複しています: {capability}"
            );
            assert!(client_api::CapabilityId::try_new(*capability).is_ok());
        }
    }
}
