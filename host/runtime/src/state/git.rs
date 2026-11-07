//! Workspace が保持する Git の状態更新。

use super::*;

impl BackendState {
    pub(crate) fn start_git_updates(
        &mut self,
        publish: Arc<dyn Fn(WorkspaceId, String, client_api::git::GitSnapshot) + Send + Sync>,
    ) {
        for (id, workspace) in &mut self.workspaces {
            if workspace.git_driver_started {
                continue;
            }
            workspace.git_driver_started = true;
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
        if workspace.removed_worktrees.contains(&repository_id) {
            return Ok(None);
        }
        if !repository_id.starts_with("worktree:")
            && workspace
                .git_repositories
                .as_ref()
                .is_some_and(|known| !known.contains(&repository_id))
        {
            return Ok(None);
        }
        if workspace
            .git_statuses
            .get(&repository_id)
            .is_some_and(|current| current.status.revision >= status.revision)
        {
            return Ok(None);
        }
        let repository = client_api::git::GitRepositoryStatus {
            repository_id: repository_id.clone(),
            status,
        };
        workspace
            .git_statuses
            .insert(repository_id, repository.clone());
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
                let id = format!("worktree:{}", tree.worktree_id.as_str());
                if tree.state == client_api::git::WorktreeState::Discarded {
                    workspace.git_statuses.remove(&id);
                    workspace.removed_worktrees.insert(id);
                } else {
                    workspace.removed_worktrees.remove(&id);
                }
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
            let known: HashSet<String> = repositories
                .iter()
                .map(|repository| repository.id.clone())
                .collect();
            let list_changed = workspace
                .git_repositories
                .as_ref()
                .is_some_and(|previous| previous != &known);
            workspace
                .git_statuses
                .retain(|id, _| id.starts_with("worktree:") || known.contains(id));
            workspace.git_repositories = Some(known);
            if list_changed {
                events.push(workspace.publish(EventBody::GitRepositoriesChanged {
                    workspace_id: workspace_id.clone(),
                })?);
            }
        }
        Ok(())
    }
}
