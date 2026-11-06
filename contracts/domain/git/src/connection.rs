use crate::{ChangeKind, GitError, WorktreeState};
use lapis_editor_core::{TaskId, WorkspaceId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitRepositorySnapshot {
    pub id: String,
    pub path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitRepositoryStatus {
    pub repository_id: String,
    pub status: GitSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitFileSnapshot {
    pub path: String,
    pub old_path: Option<String>,
    pub kind: ChangeKind,
    pub staged: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitSnapshot {
    pub workspace_id: WorkspaceId,
    pub worktree_id: Option<TaskId>,
    #[serde(default)]
    pub revision: u64,
    pub branch: String,
    pub head: String,
    pub files: Vec<GitFileSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitDiffSnapshot {
    pub path: String,
    pub additions: usize,
    pub deletions: usize,
    pub patch: String,
    pub binary: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeSnapshot {
    pub workspace_id: WorkspaceId,
    pub worktree_id: TaskId,
    pub base_commit: String,
    pub state: WorktreeState,
    pub conflict: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitUpdate {
    Snapshot(Vec<GitRepositoryStatus>),
    Changed(GitRepositoryStatus),
    RepositoriesChanged,
    WorktreesChanged,
}

pub trait GitSubscription: Send {
    fn recv_timeout(&mut self, timeout: std::time::Duration)
    -> Result<Option<GitUpdate>, GitError>;
}

pub trait GitConnection: Send + Sync {
    fn subscribe(&self, _workspace: &WorkspaceId) -> Result<Box<dyn GitSubscription>, GitError> {
        Err(GitError::new(
            crate::GitErrorKind::Io,
            "Git subscription is unsupported",
        ))
    }

    fn repositories(
        &self,
        _workspace: &WorkspaceId,
    ) -> Result<Vec<GitRepositorySnapshot>, GitError> {
        Err(GitError::new(
            crate::GitErrorKind::Io,
            "repository discovery is unsupported",
        ))
    }
    fn repository_status(
        &self,
        _workspace: &WorkspaceId,
        _repository_id: &str,
    ) -> Result<GitSnapshot, GitError> {
        Err(GitError::new(
            crate::GitErrorKind::Io,
            "repository status is unsupported",
        ))
    }
    fn repository_action(
        &self,
        _workspace: &WorkspaceId,
        _repository_id: &str,
        _request: crate::GitActionRequest,
    ) -> Result<crate::GitActionResult, GitError> {
        Err(GitError::new(
            crate::GitErrorKind::Io,
            "repository operation is unsupported",
        ))
    }

    /// 検証済みWorktreeを開き、Clientで使うroot（Remoteでは仮想path）を返す。
    fn open_worktree(
        &self,
        _workspace: &WorkspaceId,
        _worktree: &TaskId,
    ) -> Result<std::path::PathBuf, GitError> {
        Err(GitError::new(
            crate::GitErrorKind::Io,
            "Worktreeの作業環境切替に対応していません",
        ))
    }
    fn execute(
        &self,
        _workspace: &WorkspaceId,
        _worktree: Option<&TaskId>,
        _request: crate::GitActionRequest,
    ) -> Result<crate::GitActionResult, GitError> {
        Err(GitError::new(
            crate::GitErrorKind::Io,
            "Git操作に対応していません",
        ))
    }
    fn status(
        &self,
        workspace: &WorkspaceId,
        worktree: Option<&TaskId>,
    ) -> Result<GitSnapshot, GitError>;
    fn diff(
        &self,
        workspace: &WorkspaceId,
        worktree: Option<&TaskId>,
        path: &str,
    ) -> Result<GitDiffSnapshot, GitError>;
    fn worktrees(&self, workspace: &WorkspaceId) -> Result<Vec<WorktreeSnapshot>, GitError>;
    fn create_worktree(&self, workspace: &WorkspaceId) -> Result<WorktreeSnapshot, GitError>;
    fn import_file(
        &self,
        workspace: &WorkspaceId,
        worktree: &TaskId,
        path: &str,
    ) -> Result<WorktreeSnapshot, GitError>;
    fn discard_worktree(
        &self,
        workspace: &WorkspaceId,
        worktree: &TaskId,
    ) -> Result<WorktreeSnapshot, GitError>;
}
