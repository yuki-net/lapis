use serde::{Deserialize, Serialize};

use crate::{
    DocumentCloseRequest, DocumentCreateRequest, DocumentEditRequest, DocumentHistoryRequest,
    DocumentOpenRequest, DocumentSaveRequest, FileTreeRequest, SnapshotRequest,
    TerminalInputRequest, TerminalResizeRequest, TerminalStartRequest, TerminalTerminateRequest,
    WorkspaceCloseRequest, WorkspaceConnectRequest, WorkspaceId, WorkspaceListRequest, capability,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum RequestBody {
    #[serde(rename = "files.create_directory")]
    CreateDirectory(crate::CreateDirectoryRequest),
    #[serde(rename = "document.inspect")]
    DocumentInspect(crate::DocumentInspectRequest),
    #[serde(rename = "document.untitled")]
    DocumentUntitled(crate::DocumentUntitledRequest),
    #[serde(rename = "document.save_as")]
    DocumentSaveAs(crate::DocumentSaveAsRequest),
    #[serde(rename = "document.reload")]
    DocumentReload(DocumentHistoryRequest),
    #[serde(rename = "document.discard")]
    DocumentDiscard(crate::DocumentDiscardRequest),
    #[serde(rename = "workspace.execute")]
    WorkspaceExecute(crate::WorkspaceOperationRequest),
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
            Self::CreateDirectory(_) => "files.create_directory",
            Self::DocumentInspect(_) => "document.inspect",
            Self::DocumentUntitled(_) => "document.untitled",
            Self::DocumentSaveAs(_) => "document.save_as",
            Self::DocumentReload(_) => "document.reload",
            Self::DocumentDiscard(_) => "document.discard",
            Self::WorkspaceExecute(_) => "workspace.execute",
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
            Self::CreateDirectory(_) => capability::FILES_WRITE,
            Self::DocumentInspect(_) => capability::DOCUMENTS_READ,
            Self::DocumentUntitled(_)
            | Self::DocumentSaveAs(_)
            | Self::DocumentReload(_)
            | Self::DocumentDiscard(_) => capability::DOCUMENTS_WRITE,
            Self::WorkspaceExecute(request) => request.operation.required_capability(),
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

    /// Request自身が明示するWorkspace。Resource ID経由のRequestはbackend所有権表で解決する。
    pub const fn workspace_id(&self) -> Option<&WorkspaceId> {
        match self {
            Self::CreateDirectory(request) => Some(&request.workspace_id),
            Self::DocumentUntitled(request) => Some(&request.workspace_id),
            Self::DocumentInspect(_)
            | Self::DocumentSaveAs(_)
            | Self::DocumentReload(_)
            | Self::DocumentDiscard(_) => None,
            Self::WorkspaceExecute(request) => Some(&request.workspace_id),
            Self::WorkspaceList(_) => None,
            Self::WorkspaceConnect(request) => Some(&request.workspace_id),
            Self::WorkspaceClose(request) => Some(&request.workspace_id),
            Self::FileTree(request) => Some(&request.workspace_id),
            Self::DocumentOpen(request) => Some(&request.workspace_id),
            Self::DocumentCreate(request) => Some(&request.workspace_id),
            Self::TerminalStart(request) => Some(&request.workspace_id),
            Self::SnapshotResync(request) => Some(&request.workspace_id),
            Self::DocumentEdit(_)
            | Self::DocumentSave(_)
            | Self::DocumentUndo(_)
            | Self::DocumentRedo(_)
            | Self::DocumentClose(_)
            | Self::TerminalInput(_)
            | Self::TerminalResize(_)
            | Self::TerminalTerminate(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TerminalSize, WorkspaceId};

    #[test]
    fn document_discard_is_revision_checked_write_operation() {
        let request = RequestBody::DocumentDiscard(crate::DocumentDiscardRequest {
            document_id: crate::DocumentId::try_new("document-1").unwrap(),
            base_revision: crate::Revision::new(12),
        });
        assert_eq!(request.method(), "document.discard");
        assert_eq!(request.required_capability(), capability::DOCUMENTS_WRITE);
        assert_eq!(request.workspace_id(), None);
        let encoded = serde_json::to_string(&request).unwrap();
        assert_eq!(
            serde_json::from_str::<RequestBody>(&encoded).unwrap(),
            request
        );
        assert!(encoded.contains("\"base_revision\":12"));
    }

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

    #[test]
    fn direct_workspace_is_exposed_before_resource_dispatch() {
        let workspace_id = WorkspaceId::try_new("workspace-1").unwrap();
        let tree = RequestBody::FileTree(crate::FileTreeRequest {
            workspace_id: workspace_id.clone(),
            path: None,
        });
        let terminal_control = RequestBody::TerminalTerminate(crate::TerminalTerminateRequest {
            terminal_id: crate::TerminalId::try_new("terminal-1").unwrap(),
        });

        assert_eq!(tree.workspace_id(), Some(&workspace_id));
        assert_eq!(terminal_control.workspace_id(), None);
    }
}
