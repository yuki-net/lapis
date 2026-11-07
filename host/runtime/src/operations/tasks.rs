//! Task の操作。

use super::*;

impl WorkspaceOperations {
    pub(super) fn records(&self, root: &Path) -> Result<Vec<TaskSnapshot>, BackendStateError> {
        let trees = self.trees_for_tasks(root);
        Ok(self
            .tasks
            .as_ref()
            .ok_or(BackendStateError::Unsupported)?
            .load()
            .map_err(problem)?
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
    pub(super) fn allocate_ids(
        &self,
        root: &Path,
    ) -> Result<(TaskId, ExecutionId), BackendStateError> {
        let records = self
            .tasks
            .as_ref()
            .map(|tasks| tasks.load())
            .transpose()
            .map_err(problem)?
            .unwrap_or_default();
        let trees = self.trees_for_tasks(root);
        loop {
            let sequence = NEXT_RESOURCE
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                    value.checked_add(1)
                })
                .map_err(|_| BackendStateError::CounterOverflow)?;
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
    pub(super) fn task_snapshot(
        &self,
        record: TaskRecord,
        worktree_id: Option<TaskId>,
    ) -> TaskSnapshot {
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
    pub(super) fn start(
        &self,
        input: TaskStart,
        root: &Path,
    ) -> Result<Outcome, BackendStateError> {
        if input.prompt.trim().is_empty() {
            return Err(BackendStateError::WorkspaceDenied);
        }
        require_safe_id(input.conversation_id.as_str())?;
        let backend = self.tasks.as_ref().ok_or(BackendStateError::Unsupported)?;
        if input.isolated {
            self.require_repository(root)?;
        }
        let (task_id, execution_id) = self.allocate_ids(root)?;
        let tree = if input.isolated {
            let tree = self
                .git()?
                .create_worktree(root, &task_id)
                .map_err(problem)?;
            if !path_matches(&tree.repository_root, root) || tree.task_id != task_id {
                return Err(BackendStateError::WorkspaceDenied);
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
                runner: "codex-app-server".to_owned(),
                mode: input.mode,
                status: ExecutionStatus::Queued,
                started_at_ms: None,
                completed_at_ms: None,
                external_thread_id: None,
                failure: None,
            },
        );
        backend.start(&record).map_err(problem)?;
        Ok(Outcome::TaskStarted(Box::new(
            self.task_snapshot(record, tree.map(|tree| tree.task_id)),
        )))
    }
}
