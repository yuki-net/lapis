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
/// 接続した Workspace の概要。
pub struct WorkspaceConnectResponse {
    pub workspace: WorkspaceSummary,
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
