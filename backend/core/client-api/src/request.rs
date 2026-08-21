use serde::{Deserialize, Serialize};

use crate::{
    DocumentCloseRequest, DocumentCreateRequest, DocumentEditRequest, DocumentHistoryRequest,
    DocumentOpenRequest, DocumentSaveRequest, FileTreeRequest, SnapshotRequest,
    TerminalInputRequest, TerminalResizeRequest, TerminalStartRequest, TerminalTerminateRequest,
    WorkspaceCloseRequest, WorkspaceConnectRequest, WorkspaceListRequest, capability,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum RequestBody {
    #[serde(rename = "workspace.list")]
    WorkspaceList(WorkspaceListRequest),
    #[serde(rename = "workspace.connect")]
    WorkspaceConnect(WorkspaceConnectRequest),
    #[serde(rename = "workspace.close")]
    WorkspaceClose(WorkspaceCloseRequest),
    #[serde(rename = "files.tree")]
    FileTree(FileTreeRequest),
    #[serde(rename = "document.open")]
    DocumentOpen(DocumentOpenRequest),
    #[serde(rename = "document.create")]
    DocumentCreate(DocumentCreateRequest),
    #[serde(rename = "document.edit")]
    DocumentEdit(DocumentEditRequest),
    #[serde(rename = "document.save")]
    DocumentSave(DocumentSaveRequest),
    #[serde(rename = "document.undo")]
    DocumentUndo(DocumentHistoryRequest),
    #[serde(rename = "document.redo")]
    DocumentRedo(DocumentHistoryRequest),
    #[serde(rename = "document.close")]
    DocumentClose(DocumentCloseRequest),
    #[serde(rename = "terminal.start")]
    TerminalStart(TerminalStartRequest),
    #[serde(rename = "terminal.input")]
    TerminalInput(TerminalInputRequest),
    #[serde(rename = "terminal.resize")]
    TerminalResize(TerminalResizeRequest),
    #[serde(rename = "terminal.terminate")]
    TerminalTerminate(TerminalTerminateRequest),
    #[serde(rename = "snapshot.resync")]
    SnapshotResync(SnapshotRequest),
}

impl RequestBody {
    pub const fn method(&self) -> &'static str {
        match self {
            Self::WorkspaceList(_) => "workspace.list",
            Self::WorkspaceConnect(_) => "workspace.connect",
            Self::WorkspaceClose(_) => "workspace.close",
            Self::FileTree(_) => "files.tree",
            Self::DocumentOpen(_) => "document.open",
            Self::DocumentCreate(_) => "document.create",
            Self::DocumentEdit(_) => "document.edit",
            Self::DocumentSave(_) => "document.save",
            Self::DocumentUndo(_) => "document.undo",
            Self::DocumentRedo(_) => "document.redo",
            Self::DocumentClose(_) => "document.close",
            Self::TerminalStart(_) => "terminal.start",
            Self::TerminalInput(_) => "terminal.input",
            Self::TerminalResize(_) => "terminal.resize",
            Self::TerminalTerminate(_) => "terminal.terminate",
            Self::SnapshotResync(_) => "snapshot.resync",
        }
    }

    pub const fn required_capability(&self) -> &'static str {
        match self {
            Self::WorkspaceList(_) => capability::WORKSPACES,
            Self::WorkspaceConnect(_) | Self::WorkspaceClose(_) => capability::WORKSPACES_CONNECT,
            Self::FileTree(_) => capability::FILES_READ,
            Self::DocumentOpen(_) | Self::DocumentClose(_) => capability::DOCUMENTS_READ,
            Self::DocumentCreate(_)
            | Self::DocumentEdit(_)
            | Self::DocumentSave(_)
            | Self::DocumentUndo(_)
            | Self::DocumentRedo(_) => capability::DOCUMENTS_WRITE,
            Self::TerminalStart(_) => capability::TERMINAL_START,
            Self::TerminalInput(_) | Self::TerminalResize(_) | Self::TerminalTerminate(_) => {
                capability::TERMINAL_CONTROL
            }
            Self::SnapshotResync(_) => capability::WORKSPACE_SYNC,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TerminalSize, WorkspaceId};

    #[test]
    fn terminal_start_and_control_have_distinct_capabilities() {
        let start = RequestBody::TerminalStart(TerminalStartRequest {
            workspace_id: WorkspaceId::try_new("workspace-1").unwrap(),
            cwd: None,
            command: None,
            size: TerminalSize {
                columns: 80,
                rows: 24,
            },
        });
        let terminate = RequestBody::TerminalTerminate(TerminalTerminateRequest {
            terminal_id: crate::TerminalId::try_new("terminal-1").unwrap(),
        });

        assert_eq!(start.required_capability(), capability::TERMINAL_START);
        assert_eq!(
            terminate.required_capability(),
            capability::TERMINAL_CONTROL
        );
    }
}
