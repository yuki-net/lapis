use serde::{Deserialize, Serialize};

use crate::{TerminalId, TerminalOutputSequence, WorkspaceId, WorkspaceRelativePath};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSize {
    pub columns: u16,
    pub rows: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSnapshot {
    pub terminal_id: TerminalId,
    pub workspace_id: WorkspaceId,
    pub status: TerminalStatus,
    pub size: TerminalSize,
    /// backendが再同期用に保持している表示text。
    pub buffered_output: String,
    /// buffered_outputへ反映済みの最後のoutput。まだoutputがなければNone。
    pub output_watermark: Option<TerminalOutputSequence>,
    /// 保持上限によりwatermark以前のoutputが欠落している場合にtrue。
    pub output_truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStartRequest {
    pub workspace_id: WorkspaceId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<WorkspaceRelativePath>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    pub size: TerminalSize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalStartResponse {
    pub terminal: TerminalSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalInputRequest {
    pub terminal_id: TerminalId,
    pub data: String,
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
