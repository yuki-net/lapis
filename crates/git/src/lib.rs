//! Git CLI の出力を Lapis の型へ変換する境界。

use std::{fmt, path::PathBuf};

use lapis_editor_core::TaskId;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedFile {
    pub path: PathBuf,
    pub old_path: Option<PathBuf>,
    pub kind: ChangeKind,
    pub staged: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryStatus {
    pub root: PathBuf,
    pub branch: String,
    pub head: String,
    pub files: Vec<ChangedFile>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiff {
    pub path: PathBuf,
    pub additions: usize,
    pub deletions: usize,
    pub patch: String,
    pub binary: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeState {
    Active,
    Conflict,
    Integrated,
    Discarded,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskWorktree {
    pub task_id: TaskId,
    pub repository_root: PathBuf,
    pub path: PathBuf,
    pub base_commit: String,
    pub state: WorktreeState,
    pub conflict: Option<String>,
}

pub trait GitBackend: Send + Sync {
    fn status(&self, repository: &std::path::Path) -> Result<RepositoryStatus, GitError>;
    fn diff(
        &self,
        repository: &std::path::Path,
        path: &std::path::Path,
    ) -> Result<FileDiff, GitError>;
    fn create_worktree(
        &self,
        repository: &std::path::Path,
        task_id: &TaskId,
    ) -> Result<TaskWorktree, GitError>;
    fn worktrees(&self, repository: &std::path::Path) -> Result<Vec<TaskWorktree>, GitError>;
    fn import_file(
        &self,
        worktree: &TaskWorktree,
        path: &std::path::Path,
    ) -> Result<TaskWorktree, GitError>;
    fn discard_worktree(&self, worktree: &TaskWorktree) -> Result<TaskWorktree, GitError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitErrorKind {
    Io,
    NotRepository,
    Conflict,
    InvalidOutput,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitError {
    kind: GitErrorKind,
    message: String,
}

impl GitError {
    pub fn new(kind: GitErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> GitErrorKind {
        self.kind
    }
}

impl fmt::Display for GitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for GitError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_and_repository_are_separate_models() {
        let worktree = TaskWorktree {
            task_id: TaskId::new("task-1"),
            repository_root: PathBuf::from("repo"),
            path: PathBuf::from("worktree"),
            base_commit: "abc".to_owned(),
            state: WorktreeState::Active,
            conflict: None,
        };
        assert_ne!(worktree.repository_root, worktree.path);
    }
}
