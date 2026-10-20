use std::path::Path;

use lapis_document::{DocumentError, DocumentRepository};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceEntry {
    pub name: String,
    pub kind: WorkspaceEntryKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceEntryKind {
    File,
    Directory,
    Symlink,
}

/// Workspace内のDocument I/Oと直下一覧だけをbackend stateへ公開するport。
pub trait WorkspaceFileBackend: DocumentRepository {
    fn list_children(&self, directory: &Path) -> Result<Vec<WorkspaceEntry>, DocumentError>;
}
