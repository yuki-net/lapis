use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

use editor_core::{ProjectId, WorkspaceId};

mod path;
pub use path::{InvalidWorkspacePath, WorkspaceRelativePath};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceLocation {
    Local,
    Worktree { branch: String },
    Remote { authority: String },
    Container { name: String },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceCapabilities {
    pub files: bool,
    pub git: bool,
    pub lsp: bool,
    pub terminal: bool,
    pub tasks: bool,
}

impl WorkspaceCapabilities {
    pub const fn local_files() -> Self {
        Self {
            files: true,
            git: false,
            lsp: false,
            terminal: false,
            tasks: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceLifecycle {
    Open,
    Disconnected,
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub project_id: ProjectId,
    pub root: PathBuf,
    pub location: WorkspaceLocation,
    pub capabilities: WorkspaceCapabilities,
    pub lifecycle: WorkspaceLifecycle,
}

/// Host が保持する Workspace の identity。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceIdentity {
    pub id: WorkspaceId,
    pub name: String,
    /// Workspace root のディレクトリ名。name が空の場合の表示に使う。
    pub root_name: String,
}

impl WorkspaceIdentity {
    /// 表示に使う名前。Workspace 名が空なら root のディレクトリ名を使う。
    pub fn display_name(&self) -> &str {
        let name = self.name.trim();
        if name.is_empty() {
            self.root_name.trim()
        } else {
            name
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileEntryKind {
    Directory,
    File,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    pub path: PathBuf,
    pub relative_path: PathBuf,
    pub kind: FileEntryKind,
    pub depth: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHit {
    pub path: PathBuf,
    pub line: usize,
    pub utf8_column: usize,
    pub preview: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchError(String);

impl SearchError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for SearchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for SearchError {}

pub trait WorkspaceSearchBackend: Send + Sync {
    fn search(
        &self,
        root: &Path,
        query: &str,
        cancelled: &AtomicBool,
    ) -> Result<Vec<SearchHit>, SearchError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceError {
    message: String,
}

impl WorkspaceError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for WorkspaceError {}

pub trait WorkspaceRepository: Send + Sync {
    fn create_directory(&self, root: &Path, name: &str) -> Result<(), WorkspaceError>;
    fn list_tree(&self, root: &Path) -> Result<Vec<FileEntry>, WorkspaceError>;

    /// Host が保持する Workspace の identity を返す。
    ///
    /// 実装がない接続先では `None`。呼び出し側は root のディレクトリ名へ退避する。
    fn identity(&self, root: &Path) -> Result<Option<WorkspaceIdentity>, WorkspaceError> {
        let _ = root;
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_location_does_not_change_its_identity() {
        let workspace = Workspace {
            id: WorkspaceId::new("workspace-1"),
            project_id: ProjectId::new("project-1"),
            root: PathBuf::from("repo"),
            location: WorkspaceLocation::Worktree {
                branch: "task/one".to_owned(),
            },
            capabilities: WorkspaceCapabilities::local_files(),
            lifecycle: WorkspaceLifecycle::Open,
        };
        assert_eq!(workspace.id.as_str(), "workspace-1");
        assert!(matches!(
            workspace.location,
            WorkspaceLocation::Worktree { .. }
        ));
    }

    #[test]
    fn identity_display_name_uses_the_root_name_when_the_name_is_empty() {
        let identity = WorkspaceIdentity {
            id: WorkspaceId::new("workspace-1"),
            name: " ".to_owned(),
            root_name: "checkout".to_owned(),
        };
        assert_eq!(identity.display_name(), "checkout");
    }
}
