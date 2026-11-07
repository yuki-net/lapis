use serde::{Deserialize, Serialize};

use crate::{TerminalId, WorkspaceId, WorkspaceRelativePath};

pub use ::terminal::TerminalShell;
pub use ::terminal::{
    TerminalCell, TerminalColor, TerminalCursor, TerminalDelta, TerminalInput, TerminalKey,
    TerminalKeyEvent, TerminalModes, TerminalModifiers, TerminalRevision, TerminalRow,
    TerminalRowUpdate, TerminalScreen, TerminalScrollbackDelta, TerminalSize,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Terminal の実行状態。
pub enum TerminalStatus {
    Starting,
    Running,
    Exited,
    Terminated,
    Failed,
    Disconnected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Terminal の画面・状態のスナップショット。
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
/// Terminal を開始する。
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
/// 開始した Terminal の初期状態。
pub struct TerminalStartResponse {
    pub terminal: TerminalSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Terminal へ入力・操作を送る。
pub struct TerminalInputRequest {
    pub terminal_id: TerminalId,
    pub input: TerminalInput,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Terminal の表示サイズを変更する。
pub struct TerminalResizeRequest {
    pub terminal_id: TerminalId,
    pub size: TerminalSize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Terminal を終了する。
pub struct TerminalTerminateRequest {
    pub terminal_id: TerminalId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// 操作後の Terminal 状態。
pub struct TerminalCommandResponse {
    pub terminal: TerminalSnapshot,
}
