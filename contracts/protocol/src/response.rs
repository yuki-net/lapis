use serde::{Deserialize, Serialize};

use crate::{
    DocumentCloseResponse, DocumentCreateResponse, DocumentEditResponse, DocumentHistoryResponse,
    DocumentOpenResponse, DocumentSaveResponse, FileTreeResponse, ProtocolError, SnapshotResponse,
    TerminalCommandResponse, TerminalListProfilesResponse, TerminalStartResponse,
    WorkspaceCloseResponse, WorkspaceConnectResponse, WorkspaceListResponse,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
/// Host から Client への応答。tag は method 名。
pub enum ResponseBody {
    #[serde(rename = "workbench.get")]
    WorkbenchGet(crate::WorkbenchSnapshot),
    #[serde(rename = "workbench.put")]
    WorkbenchPut(crate::WorkbenchPutResponse),
    #[serde(rename = "host.worktree_path")]
    HostWorktree(String),
    #[serde(rename = "host.register_workspace")]
    HostRegister(crate::WorkspaceSummary),
    #[serde(rename = "document.relocate")]
    DocumentRelocate(crate::DocumentCloseResponse),
    #[serde(rename = "files.create_directory")]
    CreateDirectory(crate::CreateDirectoryResponse),
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
    WorkspaceExecute(crate::WorkspaceOperationResponse),
    #[serde(rename = "workspace.list")]
    WorkspaceList(WorkspaceListResponse),
    #[serde(rename = "workspace.connect")]
    WorkspaceConnect(WorkspaceConnectResponse),
    #[serde(rename = "workspace.close")]
    WorkspaceClose(WorkspaceCloseResponse),
    #[serde(rename = "workspace.view.subscribe")]
    WorkspaceViewSubscribe(crate::WorkspaceViewSnapshot),
    #[serde(rename = "workspace.view.unsubscribe")]
    WorkspaceViewUnsubscribe(crate::WorkspaceViewUnsubscribeResponse),
    #[serde(rename = "workspace.view.update")]
    WorkspaceViewUpdate(crate::WorkspaceViewUpdateResponse),
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
    #[serde(rename = "document.claim")]
    DocumentClaim(crate::DocumentCommandResponse),
    #[serde(rename = "document.release")]
    DocumentRelease(crate::DocumentCommandResponse),
    #[serde(rename = "terminal.start")]
    TerminalStart(TerminalStartResponse),
    #[serde(rename = "terminal.list_profiles")]
    TerminalListProfiles(TerminalListProfilesResponse),
    #[serde(rename = "terminal.input")]
    TerminalInput(TerminalCommandResponse),
    #[serde(rename = "terminal.resize")]
    TerminalResize(TerminalCommandResponse),
    #[serde(rename = "terminal.terminate")]
    TerminalTerminate(TerminalCommandResponse),
    #[serde(rename = "terminal.claim")]
    TerminalClaim(TerminalCommandResponse),
    #[serde(rename = "terminal.release")]
    TerminalRelease(TerminalCommandResponse),
    #[serde(rename = "snapshot.resync")]
    SnapshotResync(SnapshotResponse),
    #[serde(rename = "error")]
    Error(ProtocolError),
}

impl ResponseBody {
    pub const fn method(&self) -> Option<&'static str> {
        match self {
            Self::WorkbenchGet(_) => Some("workbench.get"),
            Self::WorkbenchPut(_) => Some("workbench.put"),
            Self::HostWorktree(_) => Some("host.worktree_path"),
            Self::HostRegister(_) => Some("host.register_workspace"),
            Self::DocumentRelocate(_) => Some("document.relocate"),
            Self::CreateDirectory(_) => Some("files.create_directory"),
            Self::DocumentInspect(_) => Some("document.inspect"),
            Self::DocumentUntitled(_) => Some("document.untitled"),
            Self::DocumentSaveAs(_) => Some("document.save_as"),
            Self::DocumentReload(_) => Some("document.reload"),
            Self::DocumentDiscard(_) => Some("document.discard"),
            Self::WorkspaceExecute(_) => Some("workspace.execute"),
            Self::WorkspaceList(_) => Some("workspace.list"),
            Self::WorkspaceConnect(_) => Some("workspace.connect"),
            Self::WorkspaceClose(_) => Some("workspace.close"),
            Self::WorkspaceViewSubscribe(_) => Some("workspace.view.subscribe"),
            Self::WorkspaceViewUnsubscribe(_) => Some("workspace.view.unsubscribe"),
            Self::WorkspaceViewUpdate(_) => Some("workspace.view.update"),
            Self::FileTree(_) => Some("files.tree"),
            Self::DocumentOpen(_) => Some("document.open"),
            Self::DocumentCreate(_) => Some("document.create"),
            Self::DocumentEdit(_) => Some("document.edit"),
            Self::DocumentSave(_) => Some("document.save"),
            Self::DocumentUndo(_) => Some("document.undo"),
            Self::DocumentRedo(_) => Some("document.redo"),
            Self::DocumentClose(_) => Some("document.close"),
            Self::DocumentClaim(_) => Some("document.claim"),
            Self::DocumentRelease(_) => Some("document.release"),
            Self::TerminalStart(_) => Some("terminal.start"),
            Self::TerminalListProfiles(_) => Some("terminal.list_profiles"),
            Self::TerminalInput(_) => Some("terminal.input"),
            Self::TerminalResize(_) => Some("terminal.resize"),
            Self::TerminalTerminate(_) => Some("terminal.terminate"),
            Self::TerminalClaim(_) => Some("terminal.claim"),
            Self::TerminalRelease(_) => Some("terminal.release"),
            Self::SnapshotResync(_) => Some("snapshot.resync"),
            Self::Error(_) => None,
        }
    }
}
