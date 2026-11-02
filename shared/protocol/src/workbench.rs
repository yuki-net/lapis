use serde::{Deserialize, Serialize};

use crate::WorkspaceId;

/// 接続中のWorkspaceに属する表示状態を取得する。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkbenchGetRequest {
    pub workspace_id: WorkspaceId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkbenchPutRequest {
    pub workspace_id: WorkspaceId,
    pub expected_revision: u64,
    pub view: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkbenchSnapshot {
    pub revision: u64,
    pub view: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "snapshot", rename_all = "snake_case")]
pub enum WorkbenchPutResponse {
    Updated(WorkbenchSnapshot),
    Conflict(WorkbenchSnapshot),
}
