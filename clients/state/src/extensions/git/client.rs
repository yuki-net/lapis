//! Client が保持する Git の状態と操作。

use std::sync::Arc;

use editor_core::{TaskId, WorkspaceId};
use git::GitError;

pub(crate) fn apply_git_status(
    current: &mut Option<git::GitSnapshot>,
    incoming: git::GitSnapshot,
) -> bool {
    if current
        .as_ref()
        .is_some_and(|status| status.revision >= incoming.revision)
    {
        return false;
    }
    *current = Some(incoming);
    true
}

pub struct GitSession {
    backend: Arc<dyn git::GitConnection>,
    workspace_id: Option<WorkspaceId>,
    status: Option<git::GitSnapshot>,
    repositories: Vec<git::GitRepositorySnapshot>,
    repository_statuses: Vec<(git::GitRepositorySnapshot, git::GitSnapshot)>,
    repository_errors: Vec<(git::GitRepositorySnapshot, GitError)>,
    last_refresh_error: Option<GitError>,
    worktrees: Vec<git::WorktreeSnapshot>,
    worktree_statuses: Vec<(TaskId, git::GitSnapshot)>,
    worktree_refreshed: Option<std::time::Instant>,
    selected_diff: Option<git::GitDiffSnapshot>,
    inspection: Option<GitDiffSelection>,
    action_output: String,
    stash_refs: Vec<(u32, String)>,
}

#[derive(Clone)]
pub struct GitDiffSelection {
    pub worktree: Option<TaskId>,
    pub repository_id: Option<String>,
    pub path: String,
    pub staged: bool,
}

impl GitSession {
    pub fn new(backend: Arc<dyn git::GitConnection>) -> Self {
        Self {
            backend,
            workspace_id: None,
            status: None,
            repositories: Vec::new(),
            repository_statuses: Vec::new(),
            repository_errors: Vec::new(),
            last_refresh_error: None,
            worktrees: Vec::new(),
            worktree_statuses: Vec::new(),
            worktree_refreshed: None,
            selected_diff: None,
            inspection: None,
            action_output: String::new(),
            stash_refs: Vec::new(),
        }
    }

    pub fn status(&self) -> Option<&git::GitSnapshot> {
        self.status.as_ref()
    }
    pub fn repository_statuses(&self) -> &[(git::GitRepositorySnapshot, git::GitSnapshot)] {
        &self.repository_statuses
    }
    pub fn repository_errors(&self) -> &[(git::GitRepositorySnapshot, GitError)] {
        &self.repository_errors
    }
    pub fn last_refresh_error(&self) -> Option<&GitError> {
        self.last_refresh_error.as_ref()
    }
    pub fn rescan_repositories(&mut self) -> Result<(), GitError> {
        let workspace = self.workspace()?.clone();
        self.repositories = self.backend.repositories(&workspace)?;
        self.refresh(&workspace)?;
        Ok(())
    }
    pub fn worktrees(&self) -> &[git::WorktreeSnapshot] {
        &self.worktrees
    }
    pub fn selected_diff(&self) -> Option<&git::GitDiffSnapshot> {
        self.selected_diff.as_ref()
    }
    pub fn worktree_statuses(&self) -> &[(TaskId, git::GitSnapshot)] {
        &self.worktree_statuses
    }

    pub fn refresh_worktrees(&mut self) -> Result<(), GitError> {
        let workspace = self.workspace()?.clone();
        let worktrees = self.backend.worktrees(&workspace)?;
        let mut statuses = Vec::new();
        for tree in worktrees
            .iter()
            .filter(|tree| tree.state != git::WorktreeState::Discarded)
        {
            if let Some((_, status)) = self
                .worktree_statuses
                .iter()
                .find(|(id, _)| id == &tree.worktree_id)
            {
                statuses.push((tree.worktree_id.clone(), status.clone()));
            } else if let Ok(status) = self.backend.status(&workspace, Some(&tree.worktree_id)) {
                statuses.push((tree.worktree_id.clone(), status));
            }
        }
        self.worktrees = worktrees;
        self.worktree_statuses = statuses;
        Ok(())
    }

    pub fn refresh(&mut self, workspace: &WorkspaceId) -> Result<bool, GitError> {
        let workspace_changed = self.workspace_id.as_ref() != Some(workspace);
        let repositories = if workspace_changed {
            self.backend.repositories(workspace)?
        } else {
            self.repositories.clone()
        };
        let mut repository_statuses = Vec::with_capacity(repositories.len());
        let mut repository_errors = Vec::new();
        for repository in &repositories {
            match self.backend.repository_status(workspace, &repository.id) {
                Ok(status) => repository_statuses.push((repository.clone(), status)),
                Err(error) => {
                    if !workspace_changed
                        && let Some((_, previous)) = self
                            .repository_statuses
                            .iter()
                            .find(|(current, _)| current.id == repository.id)
                    {
                        repository_statuses.push((repository.clone(), previous.clone()));
                    }
                    repository_errors.push((repository.clone(), error));
                }
            }
        }
        let status = repository_statuses
            .iter()
            .find(|(repository, _)| repository.id == "root")
            .map(|(_, status)| status.clone());
        let refresh_worktrees = status.is_some()
            && (workspace_changed
                || self
                    .worktree_refreshed
                    .is_none_or(|time| time.elapsed() >= std::time::Duration::from_secs(15)));
        let (worktrees, worktree_statuses) = if refresh_worktrees {
            let worktrees = self.backend.worktrees(workspace)?;
            let statuses = worktrees
                .iter()
                .filter(|worktree| worktree.state != git::WorktreeState::Discarded)
                .filter_map(|worktree| {
                    self.backend
                        .status(workspace, Some(&worktree.worktree_id))
                        .ok()
                        .map(|status| (worktree.worktree_id.clone(), status))
                })
                .collect::<Vec<_>>();
            (worktrees, statuses)
        } else if status.is_none() {
            (Vec::new(), Vec::new())
        } else {
            (self.worktrees.clone(), self.worktree_statuses.clone())
        };
        let changed = workspace_changed
            || self.status != status
            || self.repository_statuses != repository_statuses
            || self.repository_errors != repository_errors
            || self.worktrees != worktrees
            || self.worktree_statuses != worktree_statuses;
        if workspace_changed {
            self.clear_inspection();
        }
        self.workspace_id = Some(workspace.clone());
        if refresh_worktrees {
            self.worktree_refreshed = Some(std::time::Instant::now());
        }
        if status.is_none() {
            self.worktree_refreshed = None;
        }
        self.status = status;
        self.repositories = repositories;
        self.repository_statuses = repository_statuses;
        self.repository_errors = repository_errors;
        self.last_refresh_error = None;
        self.worktrees = worktrees;
        self.worktree_statuses = worktree_statuses;
        Ok(changed)
    }

    pub fn reconcile_git_update(&mut self, update: git::GitUpdate) -> bool {
        let is_snapshot = matches!(&update, git::GitUpdate::Snapshot(_));
        let mut reconciliation_error = None;
        if is_snapshot {
            if let Err(error) = self.rescan_repositories() {
                reconciliation_error = Some(error);
            }
            if self.status().is_some()
                && let Err(error) = self.refresh_worktrees()
                && reconciliation_error.is_none()
            {
                reconciliation_error = Some(error);
            }
        }
        let changed = self.apply_git_update(update);
        if is_snapshot {
            self.last_refresh_error = reconciliation_error;
        }
        changed
    }

    pub fn apply_git_update(&mut self, update: git::GitUpdate) -> bool {
        let statuses = match update {
            git::GitUpdate::Snapshot(statuses) => statuses,
            git::GitUpdate::Changed(status) => vec![status],
            git::GitUpdate::RepositoriesChanged => return false,
            git::GitUpdate::WorktreesChanged => return false,
        };
        let mut changed = false;
        for item in statuses {
            if self.workspace_id.as_ref() != Some(&item.status.workspace_id) {
                continue;
            }
            let mut repository_applied = false;
            if let Some((_, current)) = self
                .repository_statuses
                .iter_mut()
                .find(|(repository, _)| repository.id == item.repository_id)
            {
                if item.status.revision > current.revision {
                    *current = item.status.clone();
                    repository_applied = true;
                    changed = true;
                }
            } else if let Some(repository) = self
                .repositories
                .iter()
                .find(|repository| repository.id == item.repository_id)
                .cloned()
            {
                self.repository_statuses
                    .push((repository, item.status.clone()));
                repository_applied = true;
                changed = true;
            }
            if repository_applied {
                self.repository_errors
                    .retain(|(repository, _)| repository.id != item.repository_id);
            }
            if item.repository_id == "root" {
                changed |= apply_git_status(&mut self.status, item.status);
            } else if let Some(worktree) = item.status.worktree_id.as_ref()
                && let Some((_, current)) = self
                    .worktree_statuses
                    .iter_mut()
                    .find(|(id, _)| id == worktree)
                && item.status.revision > current.revision
            {
                *current = item.status;
                changed = true;
            }
        }
        changed
    }

    fn workspace(&self) -> Result<&WorkspaceId, GitError> {
        self.workspace_id.as_ref().ok_or_else(|| {
            GitError::new(git::GitErrorKind::NotRepository, "Workspace がありません")
        })
    }

    pub fn select_diff(&mut self, path: &str) -> Result<(), GitError> {
        self.selected_diff = Some(self.backend.diff(self.workspace()?, None, path)?);
        Ok(())
    }

    pub fn select_worktree_diff(&mut self, worktree: &TaskId, path: &str) -> Result<(), GitError> {
        self.selected_diff = Some(self.backend.diff(self.workspace()?, Some(worktree), path)?);
        Ok(())
    }

    pub fn import_file(&mut self, worktree: &TaskId, path: &str) -> Result<(), GitError> {
        let workspace = self.workspace()?.clone();
        self.backend.import_file(&workspace, worktree, path)?;
        self.worktree_refreshed = None;
        self.refresh(&workspace)?;
        Ok(())
    }

    pub fn discard_worktree(&mut self, worktree: &TaskId) -> Result<(), GitError> {
        let workspace = self.workspace()?.clone();
        self.backend.discard_worktree(&workspace, worktree)?;
        self.worktree_refreshed = None;
        self.refresh(&workspace)?;
        Ok(())
    }

    pub fn backend(&self) -> Arc<dyn git::GitConnection> {
        self.backend.clone()
    }

    pub fn clear_inspection(&mut self) {
        self.selected_diff = None;
        self.inspection = None;
        self.action_output.clear();
        self.stash_refs.clear();
    }
    pub fn inspection(&self) -> Option<&GitDiffSelection> {
        self.inspection.as_ref()
    }
    pub fn action_output(&self) -> &str {
        &self.action_output
    }
    pub fn stash_refs(&self) -> &[(u32, String)] {
        &self.stash_refs
    }
    pub fn create_worktree(&mut self) -> Result<(), GitError> {
        let workspace = self.workspace()?.clone();
        self.backend.create_worktree(&workspace)?;
        self.worktree_refreshed = None;
        self.refresh(&workspace)?;
        Ok(())
    }
    pub fn execute_repository_action(
        &mut self,
        repository_id: &str,
        request: git::GitActionRequest,
    ) -> Result<(), GitError> {
        let workspace = self.workspace()?.clone();
        let action = request.action.clone();
        let result = self
            .backend
            .repository_action(&workspace, repository_id, request);
        if action.is_mutation() {
            self.worktree_refreshed = None;
        }
        if let Ok(value) = &result {
            self.clear_inspection();
            self.stash_refs = value
                .stashes
                .iter()
                .map(|stash| (stash.index, stash.oid.clone()))
                .collect();
            if let git::GitAction::Diff { path, staged } = action {
                self.inspection = Some(GitDiffSelection {
                    worktree: None,
                    repository_id: Some(repository_id.to_owned()),
                    path,
                    staged,
                });
                self.selected_diff = value.diff.clone();
            } else {
                self.action_output = value.output.clone();
            }
        }
        let refreshed = self.refresh(&workspace);
        result?;
        self.last_refresh_error = refreshed.err();
        Ok(())
    }

    pub fn execute_action(
        &mut self,
        worktree: Option<&TaskId>,
        request: git::GitActionRequest,
    ) -> Result<(), GitError> {
        let workspace = self.workspace()?.clone();
        let action = request.action.clone();
        let result = self.backend.execute(&workspace, worktree, request);
        if action.is_mutation() {
            self.worktree_refreshed = None;
        }
        if let Ok(result) = &result {
            self.clear_inspection();
            if matches!(action, git::GitAction::Stashes) {
                self.stash_refs = result
                    .stashes
                    .iter()
                    .map(|stash| (stash.index, stash.oid.clone()))
                    .collect();
            }
            if let git::GitAction::Diff { path, staged } = action {
                self.inspection = Some(GitDiffSelection {
                    worktree: worktree.cloned(),
                    repository_id: None,
                    path: path.clone(),
                    staged,
                });
                self.selected_diff = result.diff.clone();
            } else {
                self.action_output = result.output.clone();
            }
        }
        // Merge/Pull等の失敗でも競合中のStatusへ更新する。
        let refreshed = self.refresh(&workspace);
        result?;
        refreshed?;
        Ok(())
    }
}
