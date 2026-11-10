use crate::{ExecutionStatus, Task, TaskControl, TaskError, TaskEventRecord, TaskMode};
use editor_core::{ConversationId, ExecutionId, TaskId, WorkspaceId};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskStart {
    pub prompt: String,
    pub mode: TaskMode,
    pub isolated: bool,
    pub conversation_id: ConversationId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionSnapshot {
    pub id: ExecutionId,
    pub task_id: TaskId,
    pub workspace_id: WorkspaceId,
    pub worktree_id: Option<TaskId>,
    pub runner: String,
    pub mode: TaskMode,
    pub status: ExecutionStatus,
    pub started_at_ms: Option<u64>,
    pub completed_at_ms: Option<u64>,
    pub external_thread_id: Option<String>,
    pub failure: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSnapshot {
    pub task: Task,
    pub execution: ExecutionSnapshot,
    pub events: Vec<TaskEventRecord>,
}

/// Host の Task 変更通知。`Ok(true)` は変化あり、`Ok(false)` は受信timeout、
/// `Err` は再同期と再購読が必要。
pub trait TaskSubscription: Send {
    fn recv_timeout(&mut self, timeout: Duration) -> Result<bool, TaskError>;
}

pub trait TaskConnection: Send + Sync {
    fn load(&self, workspace: &WorkspaceId) -> Result<Vec<TaskSnapshot>, TaskError>;
    fn start(&self, workspace: &WorkspaceId, input: TaskStart) -> Result<TaskSnapshot, TaskError>;
    fn control(
        &self,
        workspace: &WorkspaceId,
        execution: &ExecutionId,
        control: &TaskControl,
    ) -> Result<(), TaskError>;

    /// Host 側の変更通知。未対応の接続は `None`。
    fn subscribe(&self, _workspace: &WorkspaceId) -> Option<Box<dyn TaskSubscription>> {
        None
    }
}
