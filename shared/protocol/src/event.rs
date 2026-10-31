use serde::{Deserialize, Serialize};

use crate::{
    DocumentId, DocumentSnapshot, DocumentTransaction, Revision, SnapshotReason, TerminalDelta,
    TerminalId, TerminalStatus, WorkspaceId,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum EventBody {
    #[serde(rename = "workspace.changed")]
    WorkspaceChanged { workspace_id: WorkspaceId },
    #[serde(rename = "git.status_changed")]
    GitStatusChanged {
        workspace_id: WorkspaceId,
        repository: lapis_git::GitRepositoryStatus,
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
    #[serde(rename = "snapshot.required")]
    SnapshotRequired {
        workspace_id: WorkspaceId,
        reason: SnapshotReason,
    },
}
