use std::path::Path;

use document::{DocumentError, DocumentRepository};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceEntry {
    pub name: String,
    pub kind: WorkspaceEntryKind,
}

/// 外部変更の判定に使う、ファイル内容を読まないメタデータ。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileMetadata {
    pub byte_len: u64,
    pub modified_nanos: Option<u128>,
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

    /// 外部変更の判定に使うメタデータ。未対応の backend は fingerprint を使う。
    fn metadata(&self, path: &Path) -> Result<Option<FileMetadata>, DocumentError> {
        Ok(self.fingerprint(path)?.map(|fingerprint| FileMetadata {
            byte_len: fingerprint.byte_len,
            modified_nanos: fingerprint.modified_nanos,
        }))
    }

    /// Workspace 配下のファイル変更通知。未対応の backend は `None`。
    fn changes(&self) -> Option<std::sync::mpsc::Receiver<()>> {
        None
    }
}
