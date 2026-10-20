use serde::{Deserialize, Serialize};

use crate::{
    DocumentCloseResponse, DocumentCreateResponse, DocumentEditResponse, DocumentHistoryResponse,
    DocumentOpenResponse, DocumentSaveResponse, FileTreeResponse, ProtocolError, SnapshotResponse,
    TerminalCommandResponse, TerminalStartResponse, WorkspaceCloseResponse,
    WorkspaceConnectResponse, WorkspaceListResponse,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum ResponseBody {
    #[serde(rename = "document.inspect")]
    DocumentInspect(DocumentHistoryResponse),
    #[serde(rename = "document.untitled")]
    DocumentUntitled(DocumentHistoryResponse),
    #[serde(rename = "document.save_as")]
    DocumentSaveAs(DocumentHistoryResponse),
    #[serde(rename = "document.reload")]
    DocumentReload(DocumentHistoryResponse),
    #[serde(rename = "document.discard")]
    DocumentDiscard(crate::DocumentDiscardResponse),
    #[serde(rename = "workspace.execute")]
    WorkspaceExecute(crate::WorkspaceOperationResult),
    #[serde(rename = "workspace.list")]
    WorkspaceList(WorkspaceListResponse),
    #[serde(rename = "workspace.connect")]
    WorkspaceConnect(WorkspaceConnectResponse),
    #[serde(rename = "workspace.close")]
    WorkspaceClose(WorkspaceCloseResponse),
    #[serde(rename = "files.tree")]
    FileTree(FileTreeResponse),
    #[serde(rename = "document.open")]
    DocumentOpen(DocumentOpenResponse),
    #[serde(rename = "document.create")]
    DocumentCreate(DocumentCreateResponse),
    #[serde(rename = "document.edit")]
    DocumentEdit(DocumentEditResponse),
    #[serde(rename = "document.save")]
    DocumentSave(DocumentSaveResponse),
    #[serde(rename = "document.undo")]
    DocumentUndo(DocumentHistoryResponse),
    #[serde(rename = "document.redo")]
    DocumentRedo(DocumentHistoryResponse),
    #[serde(rename = "document.close")]
    DocumentClose(DocumentCloseResponse),
    #[serde(rename = "terminal.start")]
    TerminalStart(TerminalStartResponse),
    #[serde(rename = "terminal.input")]
    TerminalInput(TerminalCommandResponse),
    #[serde(rename = "terminal.resize")]
    TerminalResize(TerminalCommandResponse),
    #[serde(rename = "terminal.terminate")]
    TerminalTerminate(TerminalCommandResponse),
    #[serde(rename = "snapshot.resync")]
    SnapshotResync(SnapshotResponse),
    #[serde(rename = "error")]
    Error(ProtocolError),
}

impl ResponseBody {
    pub const fn method(&self) -> Option<&'static str> {
        match self {
            Self::DocumentInspect(_) => Some("document.inspect"),
            Self::DocumentUntitled(_) => Some("document.untitled"),
            Self::DocumentSaveAs(_) => Some("document.save_as"),
            Self::DocumentReload(_) => Some("document.reload"),
            Self::DocumentDiscard(_) => Some("document.discard"),
            Self::WorkspaceExecute(_) => Some("workspace.execute"),
            Self::WorkspaceList(_) => Some("workspace.list"),
            Self::WorkspaceConnect(_) => Some("workspace.connect"),
            Self::WorkspaceClose(_) => Some("workspace.close"),
            Self::FileTree(_) => Some("files.tree"),
            Self::DocumentOpen(_) => Some("document.open"),
            Self::DocumentCreate(_) => Some("document.create"),
            Self::DocumentEdit(_) => Some("document.edit"),
            Self::DocumentSave(_) => Some("document.save"),
            Self::DocumentUndo(_) => Some("document.undo"),
            Self::DocumentRedo(_) => Some("document.redo"),
            Self::DocumentClose(_) => Some("document.close"),
            Self::TerminalStart(_) => Some("terminal.start"),
            Self::TerminalInput(_) => Some("terminal.input"),
            Self::TerminalResize(_) => Some("terminal.resize"),
            Self::TerminalTerminate(_) => Some("terminal.terminate"),
            Self::SnapshotResync(_) => Some("snapshot.resync"),
            Self::Error(_) => None,
        }
    }
}
