use serde::{Deserialize, Serialize};

use crate::WorkspaceId;

/// Host所有者による登録要求。通常のWorkspace操作にはOSの絶対pathを渡さない。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostRegisterRequest {
    pub root: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Client に公開する Workspace の概要。
pub struct WorkspaceSummary {
    pub workspace_id: WorkspaceId,
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
/// 接続可能な Workspace の一覧を要求する。
pub struct WorkspaceListRequest {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Workspace 一覧の応答。
pub struct WorkspaceListResponse {
    pub workspaces: Vec<WorkspaceSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Workspace へ接続する。
pub struct WorkspaceConnectRequest {
    pub workspace_id: WorkspaceId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// 接続した Workspace の identity。切り替え候補の一覧とは独立して Host が返す。
pub struct WorkspaceIdentitySnapshot {
    pub workspace_id: WorkspaceId,
    pub name: String,
    /// Workspace root のディレクトリ名。name が空の場合の表示に使う。
    pub root_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// 接続した Workspace の概要と identity。
pub struct WorkspaceConnectResponse {
    pub workspace: WorkspaceSummary,
    pub identity: WorkspaceIdentitySnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Workspace から切断する。
pub struct WorkspaceCloseRequest {
    pub workspace_id: WorkspaceId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// 切断した Workspace。
pub struct WorkspaceCloseResponse {
    pub workspace_id: WorkspaceId,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_response_carries_identity_without_a_candidate_list() {
        let workspace_id = WorkspaceId::try_new("workspace-1").unwrap();
        let response = WorkspaceConnectResponse {
            workspace: WorkspaceSummary {
                workspace_id: workspace_id.clone(),
                name: "repo".to_owned(),
            },
            identity: WorkspaceIdentitySnapshot {
                workspace_id,
                name: "repo".to_owned(),
                root_name: "checkout".to_owned(),
            },
        };

        let json = serde_json::to_string(&response).unwrap();

        assert_eq!(
            json,
            r#"{"workspace":{"workspace_id":"workspace-1","name":"repo"},"identity":{"workspace_id":"workspace-1","name":"repo","root_name":"checkout"}}"#
        );
        assert_eq!(
            serde_json::from_str::<WorkspaceConnectResponse>(&json).unwrap(),
            response
        );
    }
}
