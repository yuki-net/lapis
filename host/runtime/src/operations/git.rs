//! Git の操作。

use super::*;

impl WorkspaceOperations {
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

    pub(super) fn refresh_git_event(
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

    pub(super) fn git(&self) -> Result<&dyn GitBackend, BackendStateError> {
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
    pub(super) fn trees_for_tasks(&self, root: &Path) -> Vec<TaskWorktree> {
        self.git
            .as_ref()
            .and_then(|git| git.worktrees(root).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|tree| path_matches(&tree.repository_root, root))
            .collect()
    }
    pub(super) fn require_repository(
        &self,
        directory: &Path,
    ) -> Result<RepositoryStatus, BackendStateError> {
        let status = self.git()?.status(directory).map_err(problem)?;
        if !path_matches(&status.root, directory) {
            return Err(BackendStateError::WorkspaceDenied);
        }
        Ok(status)
    }
    pub(super) fn cached_status(
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

    pub(super) fn repository_directory(
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

    pub(super) fn git_action(
        &mut self,
        directory: &Path,
        request: client_api::git::GitActionRequest,
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

    pub(super) fn git_snapshot(
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

    pub(super) fn project_git_status(
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

    pub(super) fn tree_snapshot(
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
}
