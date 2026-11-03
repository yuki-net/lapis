//! Workspaceごとの共有表示状態のwire境界。

use crate::BackendStateError;
use lapis_workbench::{ConversationViewState, VersionedViewState};

const MAX_VIEW_BYTES: usize = 1024 * 1024;

/// 保存処理は状態workerの外で実行する。
pub trait WorkbenchPersistence: Send + Sync {
    fn load(&self) -> std::io::Result<VersionedViewState>;
    fn compare_and_save(
        &self,
        expected: u64,
        view: ConversationViewState,
    ) -> std::io::Result<WorkbenchStoreResult>;
}

pub enum WorkbenchStoreResult {
    Updated(VersionedViewState),
    Conflict(VersionedViewState),
}

pub(crate) fn parse_view(
    value: serde_json::Value,
) -> Result<ConversationViewState, BackendStateError> {
    let bytes = serde_json::to_vec(&value).map_err(|_| BackendStateError::InvalidWorkbench)?;
    if bytes.len() > MAX_VIEW_BYTES {
        return Err(BackendStateError::InvalidWorkbench);
    }
    serde_json::from_value(value).map_err(|_| BackendStateError::InvalidWorkbench)
}

/// 旧Clientが知らないworkspaceを省略した場合は、Host側の値を引き継ぐ。
pub(crate) fn parse_update_view(
    value: serde_json::Value,
    current: Option<&lapis_workbench::SharedWorkspaceView>,
) -> Result<ConversationViewState, BackendStateError> {
    let omitted = value
        .as_object()
        .is_some_and(|object| !object.contains_key("workspace"));
    let mut view = parse_view(value)?;
    if omitted {
        view.workspace = current.cloned();
    }
    if serde_json::to_vec(&view)
        .map_err(|_| BackendStateError::InvalidWorkbench)?
        .len()
        > MAX_VIEW_BYTES
    {
        return Err(BackendStateError::InvalidWorkbench);
    }
    Ok(view)
}

pub(crate) fn validate_workspace(
    view: &ConversationViewState,
    workspace: &lapis_client_api::WorkspaceId,
) -> Result<(), BackendStateError> {
    if view
        .workspace
        .as_ref()
        .is_some_and(|state| state.workspace_id.as_str() != workspace.as_str())
    {
        return Err(BackendStateError::InvalidWorkbench);
    }
    Ok(())
}

pub(crate) fn snapshot(
    state: &VersionedViewState,
) -> Result<lapis_client_api::WorkbenchSnapshot, BackendStateError> {
    Ok(lapis_client_api::WorkbenchSnapshot {
        revision: state.revision,
        view: serde_json::to_value(&state.view).map_err(|_| BackendStateError::InvalidWorkbench)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_or_oversized_views() {
        assert!(parse_view(serde_json::json!({"show_panel_controls": "no"})).is_err());
        let oversized = serde_json::json!({"unknown": "x".repeat(MAX_VIEW_BYTES)});
        assert!(parse_view(oversized).is_err());
        assert!(parse_view(serde_json::json!({"show_panel_controls": false})).is_ok());
    }
}
