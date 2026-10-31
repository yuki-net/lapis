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
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

static NEXT_RESOURCE: AtomicU64 = AtomicU64::new(1);

pub(crate) struct WorkspaceOperations {
    git: Option<Arc<dyn GitBackend>>,
    tasks: Option<Arc<dyn TaskBackend>>,
    workspace_id: WorkspaceId,
    repository_cache: HashMap<PathBuf, CachedRepository>,
    next_git_revision: u64,
    notifier: Option<Arc<dyn Fn(PathBuf) + Send + Sync>>,
}

struct CachedRepository {
    watch: Option<Box<dyn lapis_git::GitWatch>>,
    status: RepositoryStatus,
    revision: u64,
    refreshed: Instant,
    worktree: Option<TaskId>,
}

const REPOSITORY_RECHECK_INTERVAL: Duration = Duration::from_secs(60);
const REPOSITORY_WATCH_RETRY_INTERVAL: Duration = Duration::from_secs(15);
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
            repository_cache: HashMap::new(),
            next_git_revision: 1,
            notifier: None,
        }
    }

    pub(crate) fn start_git_updates(
        operations: &Arc<Mutex<Self>>,
        root: PathBuf,
        publish: Arc<dyn Fn(String, GitSnapshot) + Send + Sync>,
    ) {
        let pending = Arc::new(Mutex::new(HashSet::<PathBuf>::new()));
        let (wake, receiver) = mpsc::sync_channel::<()>(1);
        let pending_callback = pending.clone();
        if let Ok(mut state) = operations.lock() {
            state.notifier = Some(Arc::new(move |path| {
                if let Ok(mut paths) = pending_callback.lock() {
                    paths.insert(path);
                }
                let _ = wake.try_send(());
            }));
        }
        let weak = Arc::downgrade(operations);
        std::thread::spawn(move || {
            loop {
                match receiver.recv_timeout(REPOSITORY_WATCH_RETRY_INTERVAL) {
                    Ok(()) => {
                        let deadline = Instant::now() + Duration::from_secs(1);
                        while Instant::now() < deadline
                            && receiver.recv_timeout(Duration::from_millis(150)).is_ok()
                        {
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        let Some(operations) = weak.upgrade() else {
                            break;
                        };
                        let Ok(state) = operations.lock() else { break };
                        if let Ok(mut paths) = pending.lock() {
                            paths.extend(
                                state
                                    .repository_cache
                                    .iter()
                                    .filter(|(_, cache)| {
                                        cache.watch.as_ref().is_none_or(|watch| !watch.is_healthy())
                                            && cache.refreshed.elapsed()
                                                >= REPOSITORY_WATCH_RETRY_INTERVAL
                                    })
                                    .map(|(path, _)| path.clone()),
                            );
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
                let paths = match pending.lock() {
                    Ok(mut paths) => std::mem::take(&mut *paths),
                    Err(_) => break,
                };
                for path in paths {
                    let Some(operations) = weak.upgrade() else {
                        return;
                    };
                    let result = operations
                        .lock()
                        .ok()
                        .and_then(|mut state| state.refresh_git_event(&root, &path).ok());
                    if let Some((id, snapshot)) = result {
                        publish(id, snapshot);
                    }
                }
            }
        });
    }

    fn refresh_git_event(
        &mut self,
        root: &Path,
        directory: &Path,
    ) -> Result<(String, GitSnapshot), BackendStateError> {
        let cache = self
            .repository_cache
            .get(directory)
            .ok_or(BackendStateError::WorkspaceDenied)?;
        let worktree = cache.worktree.clone();
        if let Some(watch) = &cache.watch {
            watch.take_changed();
        }
        let id = if let Some(worktree) = &worktree {
            format!("worktree:{}", worktree.as_str())
        } else {
            let relative = directory
                .strip_prefix(root)
                .map_err(|_| BackendStateError::WorkspaceDenied)?;
            if relative.as_os_str().is_empty() {
                "root".to_owned()
            } else {
                format!("path:{}", relative_path(relative)?)
            }
        };
        let (status, revision) = self.cached_status(directory, true)?;
        Ok((id, self.project_git_status(status, revision, worktree)?))
    }

    fn git(&self) -> Result<&dyn GitBackend, BackendStateError> {
        self.git.as_deref().ok_or(BackendStateError::Unsupported)
    }
    pub(crate) fn worktree(
        &self,
        root: &Path,
        id: &TaskId,
    ) -> Result<TaskWorktree, BackendStateError> {
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
    fn cached_status(
        &mut self,
        directory: &Path,
        force: bool,
    ) -> Result<(RepositoryStatus, u64), BackendStateError> {
        if let Some(cache) = self.repository_cache.get_mut(directory) {
            let watch_failed = cache
                .watch
                .as_ref()
                .is_some_and(|watch| !watch.is_healthy());
            if watch_failed {
                cache.watch = None;
            }
            let interval = if cache.watch.is_some() {
                REPOSITORY_RECHECK_INTERVAL
            } else {
                REPOSITORY_WATCH_RETRY_INTERVAL
            };
            if !force
                && !watch_failed
                && !cache
                    .watch
                    .as_ref()
                    .is_some_and(|watch| watch.take_changed())
                && cache.refreshed.elapsed() < interval
            {
                return Ok((cache.status.clone(), cache.revision));
            }
        }
        let watch = if self
            .repository_cache
            .get(directory)
            .is_some_and(|cache| cache.watch.is_some())
        {
            None
        } else {
            let notify = self.notifier.clone();
            let path = directory.to_owned();
            self.git()?
                .watch_repository(
                    directory,
                    Arc::new(move || {
                        if let Some(notify) = &notify {
                            notify(path.clone());
                        }
                    }),
                )
                .ok()
        };
        let status = match self.require_repository(directory) {
            Ok(status) => status,
            Err(error) => {
                if let Some(cache) = self.repository_cache.get_mut(directory) {
                    cache.watch = None;
                    cache.refreshed = Instant::now();
                }
                return Err(error);
            }
        };
        let revision = if let Some(cache) = self
            .repository_cache
            .get(directory)
            .filter(|cache| cache.status == status)
        {
            cache.revision
        } else {
            let revision = self.next_git_revision;
            self.next_git_revision = self
                .next_git_revision
                .checked_add(1)
                .ok_or(BackendStateError::CounterOverflow)?;
            revision
        };
        if let Some(cache) = self.repository_cache.get_mut(directory) {
            cache.status = status.clone();
            if cache.watch.is_none() {
                cache.watch = watch;
            }
            cache.revision = revision;
            cache.refreshed = Instant::now();
        } else {
            self.repository_cache.insert(
                directory.to_owned(),
                CachedRepository {
                    watch,
                    status: status.clone(),
                    revision,
                    refreshed: Instant::now(),
                    worktree: None,
                },
            );
        }
        Ok((status, revision))
    }

    fn repository_directory(
        &self,
        root: &Path,
        id: &str,
    ) -> Result<std::path::PathBuf, BackendStateError> {
        let resolver = crate::WorkspacePathResolver::new(root.to_owned())?;
        let relative = if id == "root" {
            None
        } else {
            Some(
                WorkspaceRelativePath::parse(
                    id.strip_prefix("path:")
                        .ok_or(BackendStateError::WorkspaceDenied)?,
                )
                .map_err(|_| BackendStateError::WorkspaceDenied)?,
            )
        };
        resolver
            .resolve_directory(relative.as_ref())
            .map_err(Into::into)
    }

    fn git_action(
        &mut self,
        directory: &Path,
        request: lapis_git::GitActionRequest,
    ) -> Result<Outcome, BackendStateError> {
        self.require_repository(directory)?;
        if let Some(path) = request.action.path() {
            let path = WorkspaceRelativePath::parse(path)
                .map_err(|_| BackendStateError::WorkspaceDenied)?;
            crate::WorkspacePathResolver::new(directory.to_owned())?.resolve_scoped_path(&path)?;
        }
        let mutation = request.action.is_mutation();
        let result = self.git()?.execute(directory, request).map_err(problem);
        if mutation {
            let _ = self.cached_status(directory, true);
            if let Some(notify) = &self.notifier {
                notify(directory.to_owned());
            }
        }
        Ok(Outcome::GitAction(result?))
    }

    fn git_snapshot(
        &mut self,
        directory: &Path,
        worktree: Option<TaskId>,
    ) -> Result<GitSnapshot, BackendStateError> {
        let initial = !self.repository_cache.contains_key(directory);
        let (status, revision) = self.cached_status(directory, false)?;
        if let Some(cache) = self.repository_cache.get_mut(directory) {
            cache.worktree = worktree.clone();
        }
        if initial && let Some(notify) = &self.notifier {
            notify(directory.to_owned());
        }
        self.project_git_status(status, revision, worktree)
    }

    fn project_git_status(
        &self,
        status: RepositoryStatus,
        revision: u64,
        worktree: Option<TaskId>,
    ) -> Result<GitSnapshot, BackendStateError> {
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
        Ok(GitSnapshot {
            workspace_id: self.workspace_id.clone(),
            worktree_id: worktree,
            revision,
            branch: status.branch,
            head: status.head,
            files,
        })
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
                    repositories.push(lapis_git::GitRepositorySnapshot {
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
                self.repository_cache.remove(&tree.path);
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
