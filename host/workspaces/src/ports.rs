use std::path::Path;

use document::{DocumentError, DocumentRepository};

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
    fn create_directory(&self, directory: &Path) -> Result<(), DocumentError>;
    fn list_children(&self, directory: &Path) -> Result<Vec<WorkspaceEntry>, DocumentError>;

    /// Workspace 配下のファイル変更通知。未対応の backend は `None`。
    fn changes(&self) -> Option<std::sync::mpsc::Receiver<()>> {
        None
    }
}
