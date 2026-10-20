use crate::BackendStateError;
use lapis_client_api::{
    WorkspaceOperation as Operation, WorkspaceOperationResult as Outcome, WorkspaceRelativePath,
};
use lapis_editor_core::{ExecutionId, TaskId, WorkspaceId};
use lapis_git::{
    GitBackend, GitDiffSnapshot, GitFileSnapshot, GitSnapshot, RepositoryStatus, TaskWorktree,
    WorktreeSnapshot,
};
use lapis_task::{
    Execution, ExecutionSnapshot, ExecutionStatus, Task, TaskBackend, TaskRecord, TaskSnapshot,
    TaskStart, unix_time_ms,
};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

static NEXT_RESOURCE: AtomicU64 = AtomicU64::new(1);

pub(crate) struct WorkspaceOperations {
    git: Option<Arc<dyn GitBackend>>,
    tasks: Option<Arc<dyn TaskBackend>>,
    workspace_id: WorkspaceId,
}
impl WorkspaceOperations {
    pub fn new(
        git: Option<Arc<dyn GitBackend>>,
        tasks: Option<Arc<dyn TaskBackend>>,
        workspace_id: &lapis_client_api::WorkspaceId,
    ) -> Self {
        Self {
            git,
            tasks,
            workspace_id: WorkspaceId::new(workspace_id.as_str()),
        }
    }

    fn git(&self) -> Result<&dyn GitBackend, BackendStateError> {
        self.git.as_deref().ok_or(BackendStateError::Unsupported)
    }
    fn worktree(&self, root: &Path, id: &TaskId) -> Result<TaskWorktree, BackendStateError> {
        require_safe_id(id.as_str())?;
        self.git()?
            .worktrees(root)
            .map_err(problem)?
            .into_iter()
            .find(|tree| &tree.task_id == id && path_matches(&tree.repository_root, root))
            .ok_or(BackendStateError::WorkspaceDenied)
    }
    fn trees_for_tasks(&self, root: &Path) -> Vec<TaskWorktree> {
        self.git
            .as_ref()
            .and_then(|git| git.worktrees(root).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|tree| path_matches(&tree.repository_root, root))
            .collect()
    }
    fn records(&self, root: &Path) -> Result<Vec<TaskSnapshot>, BackendStateError> {
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
    fn require_repository(&self, directory: &Path) -> Result<RepositoryStatus, BackendStateError> {
        let status = self.git()?.status(directory).map_err(problem)?;
        if !path_matches(&status.root, directory) {
            return Err(BackendStateError::WorkspaceDenied);
        }
        Ok(status)
    }
    fn allocate_ids(&self, root: &Path) -> Result<(TaskId, ExecutionId), BackendStateError> {
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
    fn tree_snapshot(
        &self,
        tree: TaskWorktree,
        root: &Path,
    ) -> Result<WorktreeSnapshot, BackendStateError> {
        if !path_matches(&tree.repository_root, root) {
            return Err(BackendStateError::WorkspaceDenied);
        }
        require_safe_id(tree.task_id.as_str())?;
        Ok(WorktreeSnapshot {
            workspace_id: self.workspace_id.clone(),
            worktree_id: tree.task_id,
            base_commit: tree.base_commit,
            state: tree.state,
            conflict: tree.conflict,
        })
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
    fn start(&self, input: TaskStart, root: &Path) -> Result<Outcome, BackendStateError> {
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

    pub fn execute(
        &mut self,
        operation: Operation,
        root: &Path,
    ) -> Result<Outcome, BackendStateError> {
        match operation {
            Operation::GitStatus { worktree } => {
                let directory = worktree
                    .as_ref()
                    .map(|id| self.worktree(root, id).map(|tree| tree.path))
                    .transpose()?
                    .unwrap_or_else(|| root.to_owned());
                let status = self.require_repository(&directory)?;
                let files = status
                    .files
                    .into_iter()
                    .map(|file| {
                        Ok(GitFileSnapshot {
                            path: relative_path(&file.path)?,
                            old_path: file.old_path.as_deref().map(relative_path).transpose()?,
                            kind: file.kind,
                            staged: file.staged,
                        })
                    })
                    .collect::<Result<Vec<_>, BackendStateError>>()?;
                Ok(Outcome::GitStatus(GitSnapshot {
                    workspace_id: self.workspace_id.clone(),
                    worktree_id: worktree,
                    branch: status.branch,
                    head: status.head,
                    files,
                }))
            }
            Operation::GitDiff { path, worktree } => {
                let directory = worktree
                    .as_ref()
                    .map(|id| self.worktree(root, id).map(|tree| tree.path))
                    .transpose()?
                    .unwrap_or_else(|| root.to_owned());
                self.require_repository(&directory)?;
                crate::WorkspacePathResolver::new(directory.clone())?.resolve_new_file(&path)?;
                let diff = self
                    .git()?
                    .diff(&directory, Path::new(path.as_str()))
                    .map_err(problem)?;
                Ok(Outcome::GitDiff(GitDiffSnapshot {
                    path: relative_path(&diff.path)?,
                    additions: diff.additions,
                    deletions: diff.deletions,
                    patch: diff.patch,
                    binary: diff.binary,
                }))
            }
            Operation::Worktrees => {
                self.require_repository(root)?;
                let trees = self
                    .git()?
                    .worktrees(root)
                    .map_err(problem)?
                    .into_iter()
                    .filter(|tree| path_matches(&tree.repository_root, root))
                    .map(|tree| self.tree_snapshot(tree, root))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Outcome::Worktrees(trees))
            }
            Operation::CreateWorktree => {
                self.require_repository(root)?;
                let (id, _) = self.allocate_ids(root)?;
                let tree = self.git()?.create_worktree(root, &id).map_err(problem)?;
                if tree.task_id != id {
                    return Err(BackendStateError::WorkspaceDenied);
                }
                Ok(Outcome::Worktree(self.tree_snapshot(tree, root)?))
            }
            Operation::ImportFile { worktree_id, path } => {
                let tree = self.worktree(root, &worktree_id)?;
                crate::WorkspacePathResolver::new(tree.path.clone())?.resolve_new_file(&path)?;
                crate::WorkspacePathResolver::new(root.to_owned())?.resolve_new_file(&path)?;
                let imported = self
                    .git()?
                    .import_file(&tree, Path::new(path.as_str()))
                    .map_err(problem)?;
                Ok(Outcome::Worktree(self.tree_snapshot(imported, root)?))
            }
            Operation::DiscardWorktree { worktree_id } => {
                let tree = self.worktree(root, &worktree_id)?;
                let discarded = self.git()?.discard_worktree(&tree).map_err(problem)?;
                Ok(Outcome::Worktree(self.tree_snapshot(discarded, root)?))
            }
            Operation::Tasks => Ok(Outcome::Tasks(self.records(root)?)),
            Operation::StartTask { input } => self.start(input, root),
            Operation::ControlTask {
                execution_id,
                control,
            } => {
                require_safe_id(execution_id.as_str())?;
                if !self.records(root)?.iter().any(|record| {
                    record.execution.id == execution_id && !record.execution.status.is_terminal()
                }) {
                    return Err(BackendStateError::WorkspaceDenied);
                }
                self.tasks
                    .as_ref()
                    .ok_or(BackendStateError::Unsupported)?
                    .control(&execution_id, &control)
                    .map_err(problem)?;
                Ok(Outcome::Completed)
            }
        }
    }
}
trait SafeOperationFailure {
    fn message(&self) -> &'static str;
}
impl SafeOperationFailure for lapis_git::GitError {
    fn message(&self) -> &'static str {
        match self.kind() {
            lapis_git::GitErrorKind::NotRepository => "Git repository not found",
            lapis_git::GitErrorKind::Conflict => "Git operation conflicts with current state",
            lapis_git::GitErrorKind::InvalidOutput => "Git returned invalid output",
            lapis_git::GitErrorKind::Io => "Git command failed",
        }
    }
}
impl SafeOperationFailure for lapis_task::TaskError {
    fn message(&self) -> &'static str {
        "Task execution or persistence failed"
    }
}
fn problem(error: impl SafeOperationFailure) -> BackendStateError {
    BackendStateError::Operation(error.message().to_owned())
}
fn relative_path(path: &Path) -> Result<String, BackendStateError> {
    let text = path
        .to_str()
        .ok_or(BackendStateError::WorkspaceDenied)?
        .replace('\\', "/");
    WorkspaceRelativePath::parse(text)
        .map(|path| path.as_str().to_owned())
        .map_err(|_| BackendStateError::WorkspaceDenied)
}
fn require_safe_id(id: &str) -> Result<(), BackendStateError> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|value| value.is_ascii_alphanumeric() || value == b'-' || value == b'_')
    {
        return Err(BackendStateError::WorkspaceDenied);
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

    #[test]
    fn operation_failure_messages_are_fixed_and_distinguishable() {
        for (kind, expected) in [
            (lapis_git::GitErrorKind::Io, "Git command failed"),
            (
                lapis_git::GitErrorKind::NotRepository,
                "Git repository not found",
            ),
            (
                lapis_git::GitErrorKind::Conflict,
                "Git operation conflicts with current state",
            ),
            (
                lapis_git::GitErrorKind::InvalidOutput,
                "Git returned invalid output",
            ),
        ] {
            let BackendStateError::Operation(message) =
                problem(lapis_git::GitError::new(kind, "/private/path secret"))
            else {
                panic!("Expected safe operation failure");
            };
            assert_eq!(message, expected);
        }
        let BackendStateError::Operation(message) =
            problem(lapis_task::TaskError::new("/private/path secret"))
        else {
            panic!("Expected safe task failure");
        };
        assert_eq!(message, "Task execution or persistence failed");
    }
}
