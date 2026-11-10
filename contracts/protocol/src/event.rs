use serde::{Deserialize, Serialize};

use crate::{
    DocumentId, DocumentSnapshot, DocumentTransaction, Revision, SnapshotReason, TerminalDelta,
    TerminalId, TerminalStatus, WorkspaceId,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
/// Host から Client へ送る状態変化。
pub enum EventBody {
    #[serde(rename = "workbench.changed")]
    WorkbenchChanged {
        workspace_id: WorkspaceId,
        revision: u64,
    },
    #[serde(rename = "workspace.changed")]
    WorkspaceChanged { workspace_id: WorkspaceId },
    #[serde(rename = "git.status_changed")]
    GitStatusChanged {
        workspace_id: WorkspaceId,
        repository: crate::git::GitRepositoryStatus,
    },
    #[serde(rename = "git.repositories_changed")]
    GitRepositoriesChanged { workspace_id: WorkspaceId },
    #[serde(rename = "git.worktrees_changed")]
    GitWorktreesChanged { workspace_id: WorkspaceId },
    #[serde(rename = "document.edited")]
    DocumentEdited {
        document_id: DocumentId,
        base_revision: Revision,
        revision: Revision,
        transaction: DocumentTransaction,
    },
    #[serde(rename = "document.saved")]
    DocumentSaved {
        document_id: DocumentId,
        revision: Revision,
    },
    #[serde(rename = "document.replaced")]
    DocumentReplaced { document: DocumentSnapshot },
    #[serde(rename = "document.closed")]
    DocumentClosed { document_id: DocumentId },
    #[serde(rename = "document.owner")]
    DocumentOwner {
        document_id: DocumentId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner: Option<crate::SessionId>,
    },
    #[serde(rename = "terminal.delta")]
    TerminalDelta {
        terminal_id: TerminalId,
        delta: TerminalDelta,
    },
    #[serde(rename = "terminal.status")]
    TerminalStatus {
        terminal_id: TerminalId,
        status: TerminalStatus,
    },
    #[serde(rename = "terminal.owner")]
    TerminalOwner {
        terminal_id: TerminalId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner: Option<crate::SessionId>,
    },
    #[serde(rename = "snapshot.required")]
    SnapshotRequired {
        workspace_id: WorkspaceId,
        reason: SnapshotReason,
    },
}
