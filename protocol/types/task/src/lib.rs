//! Task・Execution のデータ型。プロセス実装には依存しない。
use editor_core::{ConversationId, ExecutionId, TaskId, WorkspaceId};
use serde::{Deserialize, Serialize};

mod connection;
pub use connection::{
    ExecutionSnapshot, TaskConnection, TaskSnapshot, TaskStart, TaskSubscription,
};
use std::{
    fmt,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Queued,
    Running,
    WaitingForInput,
    WaitingForApproval,
    Succeeded,
    Failed,
    Cancelled,
}

impl ExecutionStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "待機中",
            Self::Running => "実行中",
            Self::WaitingForInput => "入力待ち",
            Self::WaitingForApproval => "承認待ち",
            Self::Succeeded => "成功",
            Self::Failed => "失敗",
            Self::Cancelled => "取消済み",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskMode {
    #[default]
    Default,
    Plan,
}

impl TaskMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Plan => "Plan",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub conversation_id: ConversationId,
    pub title: String,
    pub prompt: String,
    pub created_at_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Execution {
    pub id: ExecutionId,
    pub task_id: TaskId,
    pub workspace_id: WorkspaceId,
    pub workspace_root: PathBuf,
    pub runner: String,
    #[serde(default)]
    pub mode: TaskMode,
    pub status: ExecutionStatus,
    pub started_at_ms: Option<u64>,
    pub completed_at_ms: Option<u64>,
    pub external_thread_id: Option<String>,
    pub failure: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TaskEvent {
    StatusChanged {
        status: ExecutionStatus,
    },
    MessageDelta {
        text: String,
    },
    ReasoningDelta {
        text: String,
    },
    CommandOutput {
        text: String,
    },
    CommandCompleted {
        command: String,
        success: bool,
    },
    FileChanged {
        path: String,
    },
    InputRequested {
        request_id: String,
        question_ids: Vec<String>,
        prompt: String,
        secret: bool,
    },
    ApprovalRequested {
        request_id: String,
        summary: String,
    },
    Warning {
        message: String,
    },
    Error {
        message: String,
    },
}

impl TaskEvent {
    pub fn display_text(&self) -> String {
        match self {
            Self::StatusChanged { status } => format!("状態: {}", status.label()),
            Self::MessageDelta { text }
            | Self::ReasoningDelta { text }
            | Self::CommandOutput { text } => text.clone(),
            Self::CommandCompleted { command, success } => format!(
                "{}: {command}",
                if *success {
                    "コマンド完了"
                } else {
                    "コマンド失敗"
                }
            ),
            Self::FileChanged { path } => format!("ファイル変更: {path}"),
            Self::InputRequested { prompt, .. } => format!("入力が必要です: {prompt}"),
            Self::ApprovalRequested { summary, .. } => format!("承認が必要です: {summary}"),
            Self::Warning { message } => format!("警告: {message}"),
            Self::Error { message } => format!("エラー: {message}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskEventRecord {
    pub sequence: u64,
    pub occurred_at_ms: u64,
    pub event: TaskEvent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TaskControl {
    Cancel,
    Approve,
    Decline,
    Reply { text: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexExecutionSpec {
    pub executable: PathBuf,
    pub prompt: String,
    pub workspace_root: PathBuf,
    pub mode: TaskMode,
    /// 実行系の識別子。`None` は Codex app-server。
    #[serde(default)]
    pub runner: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskError {
    message: String,
}

impl TaskError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for TaskError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TaskError {}

impl From<std::io::Error> for TaskError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

impl From<serde_json::Error> for TaskError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(error.to_string())
    }
}

pub fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
