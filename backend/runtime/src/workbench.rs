//! Workspaceごとの共有表示状態のwire境界。

use crate::BackendStateError;
use lapis_workbench::{ConversationViewState, VersionedViewState};

const MAX_VIEW_BYTES: usize = 1024 * 1024;

pub(crate) fn parse_view(
    value: serde_json::Value,
) -> Result<ConversationViewState, BackendStateError> {
    let bytes = serde_json::to_vec(&value).map_err(|_| BackendStateError::InvalidWorkbench)?;
    if bytes.len() > MAX_VIEW_BYTES {
        return Err(BackendStateError::InvalidWorkbench);
    }
    serde_json::from_value(value).map_err(|_| BackendStateError::InvalidWorkbench)
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
