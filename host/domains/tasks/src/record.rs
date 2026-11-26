//! Task の記録と、永続化・実行を担う backend port。

use editor_core::ExecutionId;
use serde::{Deserialize, Serialize};
use task::{
    Execution, ExecutionStatus, Task, TaskControl, TaskError, TaskEvent, TaskEventRecord,
    unix_time_ms,
};

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

/// Task の永続化・実行 port。実装は `host/adapters` が持つ。
pub trait TaskBackend: Send + Sync {
    fn load(&self) -> Result<Vec<TaskRecord>, TaskError>;
    fn start(&self, record: &TaskRecord) -> Result<(), TaskError>;
    fn control(&self, execution_id: &ExecutionId, control: &TaskControl) -> Result<(), TaskError>;

    /// Task 保存領域の変更通知。未対応の backend は `None`。
    fn changes(&self) -> Option<std::sync::mpsc::Receiver<()>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::{ConversationId, TaskId, WorkspaceId};
    use std::path::PathBuf;
    use task::TaskMode;

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
