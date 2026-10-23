use serde::{Deserialize, Serialize};

use crate::{TerminalId, WorkspaceId, WorkspaceRelativePath};

pub use lapis_terminal::TerminalShell;
pub use lapis_terminal::{
    TerminalCell, TerminalColor, TerminalCursor, TerminalDelta, TerminalInput, TerminalKey,
    TerminalKeyEvent, TerminalModes, TerminalModifiers, TerminalRevision, TerminalRow,
    TerminalRowUpdate, TerminalScreen, TerminalSize,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalStatus {
    Starting,
    Running,
    Exited,
    Terminated,
    Failed,
    Disconnected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSnapshot {
    pub terminal_id: TerminalId,
    pub workspace_id: WorkspaceId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<WorkspaceRelativePath>,
    pub status: TerminalStatus,
    pub revision: TerminalRevision,
    pub screen: TerminalScreen,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStartRequest {
    pub workspace_id: WorkspaceId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<WorkspaceRelativePath>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default)]
    pub shell: TerminalShell,
    pub size: TerminalSize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStartResponse {
    pub terminal: TerminalSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalInputRequest {
    pub terminal_id: TerminalId,
    pub input: TerminalInput,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalResizeRequest {
    pub terminal_id: TerminalId,
    pub size: TerminalSize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalTerminateRequest {
    pub terminal_id: TerminalId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalCommandResponse {
    pub terminal: TerminalSnapshot,
}
