//! Task・Execution の値と操作 port。プロセス実装には依存しない。
use lapis_editor_core::{ConversationId, ExecutionId, TaskId, WorkspaceId};
use serde::{Deserialize, Serialize};

mod connection;
pub use connection::{ExecutionSnapshot, TaskConnection, TaskSnapshot, TaskStart};
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
pub struct TaskRecord {
    pub task: Task,
    pub execution: Execution,
    pub events: Vec<TaskEventRecord>,
}

impl TaskRecord {
    pub fn new(task: Task, execution: Execution) -> Self {
        Self {
            task,
            execution,
            events: Vec::new(),
        }
    }

    pub fn push_event(&mut self, event: TaskEvent) {
        if let Some(last) = self.events.last_mut() {
            let combined = match (&mut last.event, &event) {
                (TaskEvent::MessageDelta { text }, TaskEvent::MessageDelta { text: next })
                | (TaskEvent::ReasoningDelta { text }, TaskEvent::ReasoningDelta { text: next })
                | (TaskEvent::CommandOutput { text }, TaskEvent::CommandOutput { text: next }) => {
                    text.push_str(next);
                    true
                }
                _ => false,
            };
            if combined {
                last.occurred_at_ms = unix_time_ms();
                return;
            }
        }
        let sequence = self
            .events
            .last()
            .map_or(1, |record| record.sequence.saturating_add(1));
        self.events.push(TaskEventRecord {
            sequence,
            occurred_at_ms: unix_time_ms(),
            event,
        });
    }

    pub fn set_status(&mut self, status: ExecutionStatus) {
        if self.execution.status == status {
            return;
        }
        self.execution.status = status;
        if status == ExecutionStatus::Running && self.execution.started_at_ms.is_none() {
            self.execution.started_at_ms = Some(unix_time_ms());
        }
        if status.is_terminal() {
            self.execution.completed_at_ms = Some(unix_time_ms());
        }
        self.push_event(TaskEvent::StatusChanged { status });
    }
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
}

pub trait TaskBackend: Send + Sync {
    fn load(&self) -> Result<Vec<TaskRecord>, TaskError>;
    fn start(&self, record: &TaskRecord) -> Result<(), TaskError>;
    fn control(&self, execution_id: &ExecutionId, control: &TaskControl) -> Result<(), TaskError>;
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

#[cfg(test)]
mod tests {
    use super::*;
    use lapis_editor_core::{ConversationId, ExecutionId, TaskId, WorkspaceId};
    use std::path::PathBuf;

    #[test]
    fn task_state_and_event_sequence_are_monotonic() {
        let task_id = TaskId::new("task-1");
        let mut record = TaskRecord::new(
            Task {
                id: task_id.clone(),
                conversation_id: ConversationId::new("conversation-1"),
                title: "test".to_owned(),
                prompt: "prompt".to_owned(),
                created_at_ms: 1,
            },
            Execution {
                id: ExecutionId::new("execution-1"),
                task_id,
                workspace_id: WorkspaceId::new("workspace-1"),
                workspace_root: PathBuf::from("workspace"),
                runner: "codex".to_owned(),
                mode: TaskMode::Default,
                status: ExecutionStatus::Queued,
                started_at_ms: None,
                completed_at_ms: None,
                external_thread_id: None,
                failure: None,
            },
        );
        record.set_status(ExecutionStatus::Running);
        record.push_event(TaskEvent::MessageDelta {
            text: "hello".to_owned(),
        });
        record.set_status(ExecutionStatus::Succeeded);

        assert_eq!(record.execution.status, ExecutionStatus::Succeeded);
        assert_eq!(record.events.len(), 3);
        assert_eq!(record.events[0].sequence, 1);
        assert_eq!(record.events[2].sequence, 3);
        assert!(record.execution.started_at_ms.is_some());
        assert!(record.execution.completed_at_ms.is_some());
    }

    #[test]
    fn task_record_round_trip_keeps_lapis_types() {
        let task_id = TaskId::new("task-json");
        let record = TaskRecord::new(
            Task {
                id: task_id.clone(),
                conversation_id: ConversationId::new("conversation-json"),
                title: "日本語 😀".to_owned(),
                prompt: "確認".to_owned(),
                created_at_ms: 42,
            },
            Execution {
                id: ExecutionId::new("execution-json"),
                task_id,
                workspace_id: WorkspaceId::new("workspace-json"),
                workspace_root: PathBuf::from("K:/workspace"),
                runner: "codex".to_owned(),
                mode: TaskMode::Plan,
                status: ExecutionStatus::WaitingForApproval,
                started_at_ms: Some(42),
                completed_at_ms: None,
                external_thread_id: Some("thread-json".to_owned()),
                failure: None,
            },
        );
        let encoded = serde_json::to_vec(&record).unwrap();
        let decoded: TaskRecord = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, record);
    }
}
