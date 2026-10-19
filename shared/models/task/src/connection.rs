use crate::{ExecutionStatus, Task, TaskControl, TaskError, TaskEventRecord, TaskMode};
use lapis_editor_core::{ConversationId, ExecutionId, TaskId, WorkspaceId};
use serde::{Deserialize, Serialize};

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

pub trait TaskConnection: Send + Sync {
    fn load(&self, workspace: &WorkspaceId) -> Result<Vec<TaskSnapshot>, TaskError>;
    fn start(&self, workspace: &WorkspaceId, input: TaskStart) -> Result<TaskSnapshot, TaskError>;
    fn control(
        &self,
        workspace: &WorkspaceId,
        execution: &ExecutionId,
        control: &TaskControl,
    ) -> Result<(), TaskError>;
}
