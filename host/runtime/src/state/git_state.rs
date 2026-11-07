//! Workspace が保持する Git の状態更新。

use super::*;

/// Workspace ごとの Git 状態。status の重複排除と worktree の削除追跡を所有する。
#[derive(Default)]
pub(crate) struct GitState {
    statuses: HashMap<String, client_api::git::GitRepositoryStatus>,
    removed_worktrees: HashSet<String>,
    repositories: Option<HashSet<String>>,
    driver_started: bool,
}

impl GitState {
    pub(crate) fn statuses(&self) -> impl Iterator<Item = &client_api::git::GitRepositoryStatus> {
        self.statuses.values()
    }

    pub(crate) fn driver_started(&self) -> bool {
        self.driver_started
    }

    pub(crate) fn mark_driver_started(&mut self) {
        self.driver_started = true;
    }

    /// 新しい revision の status だけを受け入れ、受理した status を返す。
    fn accept(
        &mut self,
        repository_id: String,
        status: client_api::git::GitSnapshot,
    ) -> Option<client_api::git::GitRepositoryStatus> {
        if self.removed_worktrees.contains(&repository_id) {
            return None;
        }
        if !repository_id.starts_with("worktree:")
            && self
                .repositories
                .as_ref()
                .is_some_and(|known| !known.contains(&repository_id))
        {
            return None;
        }
        if self
            .statuses
            .get(&repository_id)
            .is_some_and(|current| current.status.revision >= status.revision)
        {
            return None;
        }
        let repository = client_api::git::GitRepositoryStatus {
            repository_id: repository_id.clone(),
            status,
        };
        self.statuses.insert(repository_id, repository.clone());
        Some(repository)
    }

    /// 削除された worktree の status を破棄し、それ以外の状態を追跡する。
    fn apply_worktree(&mut self, tree: &client_api::git::WorktreeSnapshot) {
        let id = format!("worktree:{}", tree.worktree_id.as_str());
        if tree.state == client_api::git::WorktreeState::Discarded {
            self.statuses.remove(&id);
            self.removed_worktrees.insert(id);
        } else {
            self.removed_worktrees.remove(&id);
        }
    }

    /// repository 一覧を反映し、一覧が変化したかを返す。
    fn apply_repositories(
        &mut self,
        repositories: &[client_api::git::GitRepositorySnapshot],
    ) -> bool {
        let known: HashSet<String> = repositories
            .iter()
            .map(|repository| repository.id.clone())
            .collect();
        let list_changed = self
            .repositories
            .as_ref()
            .is_some_and(|previous| previous != &known);
        self.statuses
            .retain(|id, _| id.starts_with("worktree:") || known.contains(id));
        self.repositories = Some(known);
        list_changed
    }
}

impl BackendState {
    pub(crate) fn start_git_updates(
        &mut self,
        publish: Arc<dyn Fn(WorkspaceId, String, client_api::git::GitSnapshot) + Send + Sync>,
    ) {
        for (id, workspace) in &mut self.workspaces {
            if workspace.git.driver_started() {
                continue;
            }
            workspace.git.mark_driver_started();
            let id = id.clone();
            let publish = publish.clone();
            crate::operations::WorkspaceOperations::start_git_updates(
                &workspace.operations,
                workspace.resolver.root().to_owned(),
                Arc::new(move |repository, status| publish(id.clone(), repository, status)),
            );
        }
    }

    pub(crate) fn publish_git_status(
        &mut self,
        workspace_id: WorkspaceId,
        repository_id: String,
        status: client_api::git::GitSnapshot,
    ) -> Result<Option<PublishedEvent>, BackendStateError> {
        let workspace = self
            .workspaces
            .get_mut(&workspace_id)
            .ok_or(BackendStateError::WorkspaceNotFound)?;
        let Some(repository) = workspace.git.accept(repository_id, status) else {
            return Ok(None);
        };
        Ok(Some(workspace.publish(EventBody::GitStatusChanged {
            workspace_id,
            repository,
        })?))
    }

    /// Workspace 操作の結果から Git の状態とイベントを更新する。
    pub(crate) fn apply_git_updates(
        &mut self,
        workspace_id: &WorkspaceId,
        result: &client_api::WorkspaceOperationResult,
        worktrees_changed: bool,
        events: &mut Vec<PublishedEvent>,
    ) -> Result<(), BackendStateError> {
        if worktrees_changed {
            let workspace = self
                .workspaces
                .get_mut(workspace_id)
                .ok_or(BackendStateError::WorkspaceNotFound)?;
            if let client_api::WorkspaceOperationResult::Worktree(tree) = result {
                workspace.git.apply_worktree(tree);
            }
            events.push(workspace.publish(EventBody::GitWorktreesChanged {
                workspace_id: workspace_id.clone(),
            })?);
        }
        if let client_api::WorkspaceOperationResult::GitRepositories(repositories) = result {
            let workspace = self
                .workspaces
                .get_mut(workspace_id)
                .ok_or(BackendStateError::WorkspaceNotFound)?;
            if workspace.git.apply_repositories(repositories) {
                events.push(workspace.publish(EventBody::GitRepositoriesChanged {
                    workspace_id: workspace_id.clone(),
                })?);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(revision: u64) -> client_api::git::GitSnapshot {
        client_api::git::GitSnapshot {
            workspace_id: editor_core::WorkspaceId::new("workspace-1"),
            worktree_id: None,
            revision,
            branch: "main".to_owned(),
            head: "abc123".to_owned(),
            files: Vec::new(),
        }
    }

    #[test]
    fn stale_status_is_rejected_after_a_newer_one() {
        let mut state = GitState::default();
        assert!(state.accept("root".to_owned(), snapshot(2)).is_some());
        assert!(state.accept("root".to_owned(), snapshot(1)).is_none());
        assert_eq!(state.statuses().count(), 1);
        assert_eq!(state.statuses().next().unwrap().status.revision, 2);
    }

    #[test]
    fn discarded_worktree_status_is_dropped_and_not_restored() {
        let mut state = GitState::default();
        let worktree = client_api::git::WorktreeSnapshot {
            workspace_id: editor_core::WorkspaceId::new("workspace-1"),
            worktree_id: editor_core::TaskId::new("task-1"),
            base_commit: "abc123".to_owned(),
            state: client_api::git::WorktreeState::Active,
            conflict: None,
        };
        assert!(
            state
                .accept("worktree:task-1".to_owned(), snapshot(1))
                .is_some()
        );

        let discarded = client_api::git::WorktreeSnapshot {
            state: client_api::git::WorktreeState::Discarded,
            ..worktree.clone()
        };
        state.apply_worktree(&discarded);

        assert!(
            state
                .accept("worktree:task-1".to_owned(), snapshot(2))
                .is_none()
        );
        assert_eq!(state.statuses().count(), 0);
    }

    #[test]
    fn unknown_repository_is_ignored_until_it_is_listed() {
        let mut state = GitState::default();
        state.apply_repositories(&[client_api::git::GitRepositorySnapshot {
            id: "root".to_owned(),
            path: None,
        }]);

        assert!(state.accept("path:other".to_owned(), snapshot(1)).is_none());
        assert!(state.accept("root".to_owned(), snapshot(1)).is_some());
    }
}
