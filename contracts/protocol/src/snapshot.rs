use serde::{Deserialize, Serialize};

use crate::{DocumentSnapshot, EventSequence, TerminalSnapshot, WorkspaceId, WorkspaceSummary};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// snapshot を要求する理由。
pub enum SnapshotReason {
    Initial,
    Reconnect,
    EventGap,
    RevisionConflict,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Workspace の全状態と event watermark。
pub struct WorkspaceSnapshot {
    /// この値以下のeventをすべて反映済みであることを示す。
    pub event_watermark: EventSequence,
    pub workspace: WorkspaceSummary,
    pub documents: Vec<DocumentSnapshot>,
    pub terminals: Vec<TerminalSnapshot>,
    #[serde(default)]
    pub git_repositories: Vec<git::GitRepositoryStatus>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Workspace snapshot を要求する。
pub struct SnapshotRequest {
    pub workspace_id: WorkspaceId,
    pub reason: SnapshotReason,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// 要求された Workspace snapshot。
pub struct SnapshotResponse {
    pub snapshot: WorkspaceSnapshot,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_carries_the_last_applied_event_watermark() {
        let snapshot = WorkspaceSnapshot {
            event_watermark: EventSequence::new(42),
            workspace: WorkspaceSummary {
                workspace_id: WorkspaceId::try_new("workspace-1").unwrap(),
                name: "Lapis".to_owned(),
            },
            documents: Vec::new(),
            terminals: Vec::new(),
            git_repositories: Vec::new(),
        };

        let restored =
            serde_json::from_str::<WorkspaceSnapshot>(&serde_json::to_string(&snapshot).unwrap())
                .unwrap();
        assert_eq!(restored.event_watermark, EventSequence::new(42));
    }
}
