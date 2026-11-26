//! Task のオーケストレーション。状態の正本は `host/runtime` が持つ。

use std::{
    error::Error,
    fmt,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use editor_core::{ExecutionId, TaskId, WorkspaceId};
use git::TaskWorktree;
use task::{
    Execution, ExecutionSnapshot, ExecutionStatus, Task, TaskBackend, TaskControl, TaskRecord,
    TaskSnapshot, TaskStart, unix_time_ms,
};

static NEXT_RESOURCE: AtomicU64 = AtomicU64::new(1);

/// Task 操作で発生するエラー。
#[derive(Debug)]
pub enum TaskStateError {
    /// Task backend が接続されていない。
    Unsupported,
    /// 入力または対象が Workspace の範囲外。
    Denied,
    /// 連番資源が枯渇した。
    CounterOverflow,
    /// Task backend の失敗。
    Backend(task::TaskError),
    /// 操作に必要な外部処理の失敗（固定メッセージ）。
    Operation(String),
}

impl fmt::Display for TaskStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Unsupported => "Task execution or persistence failed",
            Self::Denied => "Task request is outside the workspace",
            Self::CounterOverflow => "Task resource counter overflowed",
            Self::Backend(_) | Self::Operation(_) => "Task execution or persistence failed",
        };
        formatter.write_str(message)
    }
}

impl Error for TaskStateError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Backend(error) => Some(error),
            _ => None,
        }
    }
}

impl From<task::TaskError> for TaskStateError {
    fn from(error: task::TaskError) -> Self {
        Self::Backend(error)
    }
}

/// Workspace 1つ分の Task 操作。
pub struct TaskOperations {
    tasks: Option<Arc<dyn TaskBackend>>,
    workspace_id: WorkspaceId,
}

impl TaskOperations {
    pub fn new(tasks: Option<Arc<dyn TaskBackend>>, workspace_id: WorkspaceId) -> Self {
        Self {
            tasks,
            workspace_id,
        }
    }

    /// Workspace に属する Task の snapshot を返す。
    pub fn records(
        &self,
        root: &Path,
        trees: &[TaskWorktree],
    ) -> Result<Vec<TaskSnapshot>, TaskStateError> {
        let backend = self.tasks.as_ref().ok_or(TaskStateError::Unsupported)?;
        Ok(backend
            .load()?
            .into_iter()
            .filter_map(|record| {
                let worktree = trees
                    .iter()
                    .find(|tree| path_matches(&tree.path, &record.execution.workspace_root));
                if !path_matches(&record.execution.workspace_root, root) && worktree.is_none() {
                    return None;
                }
                Some(self.task_snapshot(record, worktree.map(|tree| tree.task_id.clone())))
            })
            .collect())
    }

    /// 既存 Task と衝突しない ID を割り当てる。
    pub fn allocate_ids(
        &self,
        trees: &[TaskWorktree],
    ) -> Result<(TaskId, ExecutionId), TaskStateError> {
        let records = self
            .tasks
            .as_ref()
            .map(|tasks| tasks.load())
            .transpose()?
            .unwrap_or_default();
        loop {
            let sequence = NEXT_RESOURCE
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                    value.checked_add(1)
                })
                .map_err(|_| TaskStateError::CounterOverflow)?;
            let suffix = format!("{}-{}-{sequence}", unix_time_ms(), std::process::id());
            let task = TaskId::new(format!("task-{suffix}"));
            let execution = ExecutionId::new(format!("execution-{suffix}"));
            if records
                .iter()
                .any(|record| record.task.id == task || record.execution.id == execution)
                || trees.iter().any(|tree| tree.task_id == task)
            {
                continue;
            }
            return Ok((task, execution));
        }
    }

    /// Task を開始する。分離実行の worktree 作成は呼び出し側から渡す。
    pub fn start(
        &self,
        input: TaskStart,
        root: &Path,
        trees: impl FnOnce() -> Vec<TaskWorktree>,
        create_worktree: impl FnOnce(&TaskId) -> Result<TaskWorktree, TaskStateError>,
    ) -> Result<TaskSnapshot, TaskStateError> {
        validate_start(&input)?;
        let backend = self.tasks.as_ref().ok_or(TaskStateError::Unsupported)?;
        let trees = trees();
        let (task_id, execution_id) = self.allocate_ids(&trees)?;
        let tree = if input.isolated {
            let tree = create_worktree(&task_id)?;
            if !path_matches(&tree.repository_root, root) || tree.task_id != task_id {
                return Err(TaskStateError::Denied);
            }
            Some(tree)
        } else {
            None
        };
        let record = TaskRecord::new(
            Task {
                id: task_id.clone(),
                conversation_id: input.conversation_id,
                title: input
                    .prompt
                    .lines()
                    .next()
                    .unwrap_or("Task")
                    .chars()
                    .take(48)
                    .collect(),
                prompt: input.prompt,
                created_at_ms: unix_time_ms(),
            },
            Execution {
                id: execution_id,
                task_id,
                workspace_id: self.workspace_id.clone(),
                workspace_root: tree
                    .as_ref()
                    .map(|tree| tree.path.clone())
                    .unwrap_or_else(|| root.to_owned()),
                runner: input
                    .runner
                    .clone()
                    .unwrap_or_else(|| "codex-app-server".to_owned()),
                mode: input.mode,
                status: ExecutionStatus::Queued,
                started_at_ms: None,
                completed_at_ms: None,
                external_thread_id: None,
                failure: None,
            },
        );
        backend.start(&record)?;
        Ok(self.task_snapshot(record, tree.map(|tree| tree.task_id)))
    }

    /// 実行中の Task を操作する。対象が実行中でなければ拒否する。
    pub fn control(
        &self,
        execution_id: &ExecutionId,
        control: &TaskControl,
    ) -> Result<(), TaskStateError> {
        require_safe_id(execution_id.as_str())?;
        self.tasks
            .as_ref()
            .ok_or(TaskStateError::Unsupported)?
            .control(execution_id, control)?;
        Ok(())
    }

    fn task_snapshot(&self, record: TaskRecord, worktree_id: Option<TaskId>) -> TaskSnapshot {
        let execution = record.execution;
        TaskSnapshot {
            task: record.task,
            events: record.events,
            execution: ExecutionSnapshot {
                id: execution.id,
                task_id: execution.task_id,
                workspace_id: self.workspace_id.clone(),
                worktree_id,
                runner: execution.runner,
                mode: execution.mode,
                status: execution.status,
                started_at_ms: execution.started_at_ms,
                completed_at_ms: execution.completed_at_ms,
                external_thread_id: execution.external_thread_id,
                failure: execution.failure,
            },
        }
    }
}

/// Task 開始要求を、副作用のない範囲で検証する。
pub fn validate_start(input: &TaskStart) -> Result<(), TaskStateError> {
    if input.prompt.trim().is_empty() {
        return Err(TaskStateError::Denied);
    }
    if let Some(runner) = &input.runner
        && runner.len() > 64
    {
        return Err(TaskStateError::Denied);
    }
    require_safe_id(input.conversation_id.as_str())
}

/// Task を開始できる実行中状態かを判定する。
pub fn is_running(snapshot: &TaskSnapshot) -> bool {
    !snapshot.execution.status.is_terminal()
}

fn require_safe_id(id: &str) -> Result<(), TaskStateError> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|value| value.is_ascii_alphanumeric() || value == b'-' || value == b'_')
    {
        return Err(TaskStateError::Denied);
    }
    Ok(())
}

fn path_matches(left: &Path, right: &Path) -> bool {
    left == right
        || left
            .canonicalize()
            .is_ok_and(|left| right.canonicalize().is_ok_and(|right| left == right))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Fixture {
        records: Mutex<Vec<TaskRecord>>,
        started: Mutex<Vec<TaskId>>,
        controlled: Mutex<Vec<(ExecutionId, TaskControl)>>,
    }

    impl TaskBackend for Fixture {
        fn load(&self) -> Result<Vec<TaskRecord>, task::TaskError> {
            Ok(self.records.lock().unwrap().clone())
        }

        fn start(&self, record: &TaskRecord) -> Result<(), task::TaskError> {
            self.started.lock().unwrap().push(record.task.id.clone());
            self.records.lock().unwrap().push(record.clone());
            Ok(())
        }

        fn control(
            &self,
            execution_id: &ExecutionId,
            control: &TaskControl,
        ) -> Result<(), task::TaskError> {
            self.controlled
                .lock()
                .unwrap()
                .push((execution_id.clone(), control.clone()));
            Ok(())
        }
    }

    fn operations() -> (TaskOperations, Arc<Fixture>) {
        let fixture = Arc::new(Fixture::default());
        (
            TaskOperations::new(Some(fixture.clone()), WorkspaceId::new("workspace-1")),
            fixture,
        )
    }

    fn input(prompt: &str, isolated: bool) -> TaskStart {
        TaskStart {
            prompt: prompt.to_owned(),
            mode: task::TaskMode::Plan,
            isolated,
            conversation_id: editor_core::ConversationId::new("conversation-1"),
            runner: None,
        }
    }

    #[test]
    fn start_assigns_unique_ids_and_queues_the_task() {
        let (operations, fixture) = operations();
        let root = std::env::temp_dir();
        let first = operations
            .start(input("first", false), &root, Vec::new, |_| {
                unreachable!("not isolated")
            })
            .unwrap();
        assert_eq!(first.execution.status, ExecutionStatus::Queued);
        assert!(first.execution.worktree_id.is_none());
        assert_eq!(first.execution.workspace_id.as_str(), "workspace-1");

        let second = operations
            .start(input("second", false), &root, Vec::new, |_| {
                unreachable!("not isolated")
            })
            .unwrap();
        assert_ne!(first.task.id, second.task.id);
        assert_ne!(first.execution.id, second.execution.id);
        assert_eq!(fixture.started.lock().unwrap().len(), 2);
    }

    #[test]
    fn empty_prompt_and_unsafe_conversation_id_are_rejected() {
        let (operations, _) = operations();
        let root = std::env::temp_dir();
        let error = operations
            .start(input("   ", false), &root, Vec::new, |_| unreachable!())
            .unwrap_err();
        assert!(matches!(error, TaskStateError::Denied));

        let mut unsafe_input = input("prompt", false);
        unsafe_input.conversation_id = editor_core::ConversationId::new("../escape");
        let error = operations
            .start(unsafe_input, &root, Vec::new, |_| unreachable!())
            .unwrap_err();
        assert!(matches!(error, TaskStateError::Denied));
    }

    #[test]
    fn isolated_start_uses_the_created_worktree() {
        let (operations, _) = operations();
        let root = std::env::temp_dir();
        let snapshot = operations
            .start(input("isolated", true), &root, Vec::new, |task_id| {
                Ok(TaskWorktree {
                    task_id: task_id.clone(),
                    repository_root: root.clone(),
                    path: root.join(task_id.as_str()),
                    base_commit: "abc".to_owned(),
                    state: git::WorktreeState::Active,
                    conflict: None,
                })
            })
            .unwrap();
        assert_eq!(
            snapshot.execution.worktree_id.as_ref(),
            Some(&snapshot.task.id)
        );
    }

    #[test]
    fn control_rejects_unsupported_backend() {
        let operations = TaskOperations::new(None, WorkspaceId::new("workspace-1"));
        let error = operations
            .control(&ExecutionId::new("execution-1"), &TaskControl::Cancel)
            .unwrap_err();
        assert!(matches!(error, TaskStateError::Unsupported));
    }
}
