use std::{
    error::Error,
    fmt,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

use lapis_editor_core::{ProjectId, WorkspaceId};

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
    /// RemoteのHost所有者が公開したソース。Localでは通常のfolder pickerを使う。
    fn available_sources(&self) -> Result<Vec<(PathBuf, String)>, WorkspaceError> {
        Ok(Vec::new())
    }
    fn create_directory(&self, root: &Path, name: &str) -> Result<(), WorkspaceError>;
    fn list_tree(&self, root: &Path) -> Result<Vec<FileEntry>, WorkspaceError>;
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
}
