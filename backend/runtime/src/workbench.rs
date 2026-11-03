//! Workspaceごとの共有表示状態のwire境界。

use crate::BackendStateError;
use lapis_workbench::{ConversationViewState, VersionedViewState};

/// 画面配置に許す容量。未保存下書きは別枠で数える。
const MAX_LAYOUT_BYTES: usize = 1024 * 1024;
/// 1文書の未保存下書きに許す容量。
const MAX_DRAFT_BYTES: usize = 4 * 1024 * 1024;
/// 1回のWorkbench更新全体に許す容量。Remote transportのmessage上限(16 MiB)未満に保つ。
const MAX_VIEW_BYTES: usize = 14 * 1024 * 1024;

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
    validate_value(&value)?;
    serde_json::from_value(value).map_err(|_| BackendStateError::InvalidWorkbench)
}

/// 画面配置と下書きを別々の容量で検証する。下書きが大きくても配置の更新を妨げない。
fn validate_value(value: &serde_json::Value) -> Result<(), BackendStateError> {
    let bytes = serde_json::to_vec(value).map_err(|_| BackendStateError::InvalidWorkbench)?;
    if bytes.len() > MAX_VIEW_BYTES {
        return Err(BackendStateError::InvalidWorkbench);
    }
    let mut layout = value.clone();
    if let Some(documents) = layout
        .get_mut("workspace")
        .and_then(|workspace| workspace.get_mut("open_documents"))
        .and_then(|documents| documents.as_array_mut())
    {
        for document in documents {
            if let Some(document) = document.as_object_mut() {
                document.remove("draft_content");
            }
        }
    }
    if serde_json::to_vec(&layout)
        .map_err(|_| BackendStateError::InvalidWorkbench)?
        .len()
        > MAX_LAYOUT_BYTES
    {
        return Err(BackendStateError::InvalidWorkbench);
    }
    let oversized_draft = value
        .get("workspace")
        .and_then(|workspace| workspace.get("open_documents"))
        .and_then(|documents| documents.as_array())
        .is_some_and(|documents| {
            documents.iter().any(|document| {
                document
                    .get("draft_content")
                    .and_then(|draft| draft.as_str())
                    .is_some_and(|draft| draft.len() > MAX_DRAFT_BYTES)
            })
        });
    if oversized_draft {
        return Err(BackendStateError::InvalidWorkbench);
    }
    Ok(())
}

fn validate_view_bytes(view: &ConversationViewState) -> Result<(), BackendStateError> {
    let value = serde_json::to_value(view).map_err(|_| BackendStateError::InvalidWorkbench)?;
    validate_value(&value)
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
    validate_view_bytes(&view)?;
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

    fn workspace_with_drafts(drafts: impl IntoIterator<Item = String>) -> serde_json::Value {
        let documents: Vec<_> = drafts
            .into_iter()
            .map(|draft| {
                serde_json::json!({
                    "document_id": null,
                    "path": null,
                    "cursor_char": 0,
                    "selection_start": 0,
                    "selection_end": 0,
                    "scroll_x": 0.0,
                    "scroll_y": 0.0,
                    "draft_content": draft,
                })
            })
            .collect();
        serde_json::json!({
            "workspace": {
                "workspace_id": "workspace-1",
                "open_documents": documents,
                "active_path": null,
                "active_document_id": null,
            }
        })
    }

    #[test]
    fn rejects_invalid_or_oversized_views() {
        assert!(parse_view(serde_json::json!({"show_panel_controls": "no"})).is_err());
        let oversized = serde_json::json!({"unknown": "x".repeat(MAX_LAYOUT_BYTES)});
        assert!(parse_view(oversized).is_err());
        assert!(parse_view(serde_json::json!({"show_panel_controls": false})).is_ok());
    }

    #[test]
    fn large_drafts_do_not_consume_the_layout_budget() {
        let draft = "x".repeat(3 * 1024 * 1024);
        let view = parse_view(workspace_with_drafts([draft.clone()])).unwrap();
        assert_eq!(
            view.workspace.unwrap().open_documents[0].draft_content,
            Some(draft)
        );
    }

    #[test]
    fn draft_larger_than_its_own_limit_is_rejected() {
        let oversized = "x".repeat(MAX_DRAFT_BYTES + 1);
        assert!(parse_view(workspace_with_drafts([oversized])).is_err());
    }

    #[test]
    fn total_view_is_bounded_independently_of_the_draft_limit() {
        let drafts = (0..4).map(|_| "x".repeat(MAX_DRAFT_BYTES));
        assert!(parse_view(workspace_with_drafts(drafts)).is_err());
    }
}
