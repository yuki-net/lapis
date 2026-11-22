use serde::{Deserialize, Serialize};

use crate::WorkspaceId;

/// 接続中のWorkspaceに属する表示状態を取得する。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkbenchGetRequest {
    pub workspace_id: WorkspaceId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Workbench 表示状態を期待 revision 付きで更新する。
pub struct WorkbenchPutRequest {
    pub workspace_id: WorkspaceId,
    pub expected_revision: u64,
    pub view: serde_json::Value,
}

/// 新Clientはworkspaceを明示し、旧Clientが省略した場合と区別する。
pub fn encode_workbench_view(
    view: &workbench::ConversationViewState,
) -> Result<serde_json::Value, serde_json::Error> {
    let mut value = serde_json::to_value(view)?;
    if view.workspace.is_none() {
        value
            .as_object_mut()
            .expect("Workbench view must be an object")
            .insert("workspace".into(), serde_json::Value::Null);
    }
    Ok(value)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Workbench 表示状態とその revision。
pub struct WorkbenchSnapshot {
    pub revision: u64,
    pub view: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "snapshot", rename_all = "snake_case")]
/// Workbench 更新の結果。競合時は最新 snapshot を返す。
pub enum WorkbenchPutResponse {
    Updated(WorkbenchSnapshot),
    Conflict(WorkbenchSnapshot),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_client_explicitly_encodes_workspace_clear() {
        let encoded = encode_workbench_view(&workbench::ConversationViewState::default()).unwrap();
        assert_eq!(encoded.get("workspace"), Some(&serde_json::Value::Null));
    }
}
