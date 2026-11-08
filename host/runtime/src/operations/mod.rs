use crate::BackendStateError;
use client_api::git::{
    GitBackend, GitDiffSnapshot, GitFileSnapshot, GitSnapshot, RepositoryStatus, TaskWorktree,
    WorktreeSnapshot,
};
use client_api::{
    WorkspaceOperation as Operation, WorkspaceOperationResponse as Outcome, WorkspaceRelativePath,
};
use editor_core::{ExecutionId, TaskId, WorkspaceId};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};
use task::{TaskBackend, TaskControl, TaskSnapshot, TaskStart};

pub(crate) struct WorkspaceOperations {
    git: Option<Arc<dyn GitBackend>>,
    tasks: task_state::TaskOperations,
    workspace_id: WorkspaceId,
    repository_cache: HashMap<PathBuf, CachedRepository>,
    next_git_revision: u64,
    notifier: Option<Arc<dyn Fn(PathBuf) + Send + Sync>>,
}

struct CachedRepository {
    watch: Option<Box<dyn client_api::git::GitWatch>>,
    status: RepositoryStatus,
    revision: u64,
    refreshed: Instant,
    worktree: Option<TaskId>,
}

const REPOSITORY_RECHECK_INTERVAL: Duration = Duration::from_secs(60);
const REPOSITORY_WATCH_RETRY_INTERVAL: Duration = Duration::from_secs(15);
mod git;
mod tasks;

impl WorkspaceOperations {
    pub fn new(
        git: Option<Arc<dyn GitBackend>>,
        tasks: Option<Arc<dyn TaskBackend>>,
        workspace_id: &client_api::WorkspaceId,
    ) -> Self {
        Self {
            git,
            tasks: task_state::TaskOperations::new(tasks, WorkspaceId::new(workspace_id.as_str())),
            workspace_id: WorkspaceId::new(workspace_id.as_str()),
            repository_cache: HashMap::new(),
            next_git_revision: 1,
            notifier: None,
        }
    }
    pub fn execute(
        &mut self,
        operation: Operation,
        root: &Path,
    ) -> Result<Outcome, BackendStateError> {
        match operation {
            Operation::GitRepositories => {
                let resolver = crate::WorkspacePathResolver::new(root.to_owned())?;
                let mut repositories = Vec::new();
                let mut discovered = HashSet::new();
                for path in self
                    .git()?
                    .discover_repositories(resolver.root())
                    .map_err(problem)?
                {
                    let relative = path
                        .strip_prefix(resolver.root())
                        .map_err(|_| BackendStateError::WorkspaceDenied)?;
                    let path = if relative.as_os_str().is_empty() {
                        None
                    } else {
                        Some(relative_path(relative)?)
                    };
                    let checked = path
                        .as_deref()
                        .map(WorkspaceRelativePath::parse)
                        .transpose()
                        .map_err(|_| BackendStateError::WorkspaceDenied)?;
                    let directory = resolver.resolve_directory(checked.as_ref())?;
                    if !path_matches(&directory, &self.require_repository(&directory)?.root) {
                        return Err(BackendStateError::WorkspaceDenied);
                    }
                    discovered.insert(directory);
                    repositories.push(client_api::git::GitRepositorySnapshot {
                        id: path
                            .as_ref()
                            .map_or_else(|| "root".to_owned(), |path| format!("path:{path}")),
                        path,
                    });
                }
                self.repository_cache
                    .retain(|path, cache| cache.worktree.is_some() || discovered.contains(path));
                Ok(Outcome::GitRepositories(repositories))
            }
            Operation::RepositoryGitStatus { repository_id } => {
                let directory = self.repository_directory(root, &repository_id)?;
                Ok(Outcome::RepositoryGitStatus {
                    repository_id,
                    status: self.git_snapshot(&directory, None)?,
                })
            }
            Operation::RepositoryGitAction {
                repository_id,
                request,
            } => {
                let directory = self.repository_directory(root, &repository_id)?;
                self.git_action(&directory, request)
            }
            Operation::GitAction { worktree, request } => {
                let directory = worktree
                    .as_ref()
                    .map(|id| self.worktree(root, id).map(|tree| tree.path))
                    .transpose()?
                    .unwrap_or_else(|| root.to_owned());
                self.git_action(&directory, request)
            }
            Operation::GitStatus { worktree } => {
                let directory = worktree
                    .as_ref()
                    .map(|id| self.worktree(root, id).map(|tree| tree.path))
                    .transpose()?
                    .unwrap_or_else(|| root.to_owned());
                Ok(Outcome::GitStatus(self.git_snapshot(&directory, worktree)?))
            }
            Operation::GitDiff { path, worktree } => {
                let directory = worktree
                    .as_ref()
                    .map(|id| self.worktree(root, id).map(|tree| tree.path))
                    .transpose()?
                    .unwrap_or_else(|| root.to_owned());
                self.require_repository(&directory)?;
                crate::WorkspacePathResolver::new(directory.clone())?.resolve_scoped_path(&path)?;
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
                let (id, _) = self.tasks.allocate_ids(root, &self.trees_for_tasks(root))?;
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
                self.repository_cache.remove(&tree.path);
                Ok(Outcome::Worktree(self.tree_snapshot(discarded, root)?))
            }
            Operation::Tasks => Ok(Outcome::Tasks(self.records(root)?)),
            Operation::StartTask { input } => self.start(input, root),
            Operation::ControlTask {
                execution_id,
                control,
            } => {
                self.control(&execution_id, &control, root)?;
                Ok(Outcome::Completed)
            }
        }
    }
}
trait SafeOperationFailure {
    fn message(&self) -> &'static str;
}
impl SafeOperationFailure for client_api::git::GitError {
    fn message(&self) -> &'static str {
        match self.kind() {
            client_api::git::GitErrorKind::NotRepository => "Git repository not found",
            client_api::git::GitErrorKind::Conflict => "Git operation conflicts with current state",
            client_api::git::GitErrorKind::InvalidOutput => "Git returned invalid output",
            client_api::git::GitErrorKind::Io => "Git command failed",
        }
    }
}
impl SafeOperationFailure for task::TaskError {
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
            (client_api::git::GitErrorKind::Io, "Git command failed"),
            (
                client_api::git::GitErrorKind::NotRepository,
                "Git repository not found",
            ),
            (
                client_api::git::GitErrorKind::Conflict,
                "Git operation conflicts with current state",
            ),
            (
                client_api::git::GitErrorKind::InvalidOutput,
                "Git returned invalid output",
            ),
        ] {
            let BackendStateError::Operation(message) =
                problem(client_api::git::GitError::new(kind, "/private/path secret"))
            else {
                panic!("Expected safe operation failure");
            };
            assert_eq!(message, expected);
        }
        let BackendStateError::Operation(message) =
            problem(task::TaskError::new("/private/path secret"))
        else {
            panic!("Expected safe task failure");
        };
        assert_eq!(message, "Task execution or persistence failed");
    }
}
