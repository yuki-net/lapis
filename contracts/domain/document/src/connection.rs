use crate::{Document, DocumentError, ExternalChange, Revision};
use editor_core::{DocumentId, SessionId};
use std::{
    ops::Range,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct DocumentCache {
    pub id: DocumentId,
    pub document: Document,
    pub can_undo: bool,
    pub can_redo: bool,
    pub external_change: ExternalChange,
    pub reused: bool,
    /// 編集を許可されている Session。`None` は未所有。
    pub owner: Option<SessionId>,
    /// この cache を作った接続自身が owner か。
    pub is_owner: bool,
}

impl DocumentCache {
    /// 他の Session が所有しているため編集できないか。
    pub fn is_read_only(&self) -> bool {
        self.owner.is_some() && !self.is_owner
    }
}

pub trait DocumentConnection: Send + Sync {
    fn fork(&self) -> std::sync::Arc<dyn DocumentConnection>;
    fn workspace(&self, root: &Path) -> Result<editor_core::WorkspaceId, DocumentError>;
    fn create(&self, root: Option<&Path>) -> Result<DocumentCache, DocumentError>;
    fn open(&self, path: &Path) -> Result<DocumentCache, DocumentError>;
    fn attach(&self, id: &DocumentId) -> Result<DocumentCache, DocumentError>;
    /// 編集権を取得する。他の Session が所有中なら奪う。
    fn claim(&self, id: &DocumentId) -> Result<DocumentCache, DocumentError> {
        let _ = id;
        Err(DocumentError::io(
            "この接続ではDocumentの編集権を取得できません",
        ))
    }
    /// 編集権を手放す。
    fn release(&self, id: &DocumentId) -> Result<DocumentCache, DocumentError> {
        let _ = id;
        Err(DocumentError::io(
            "この接続ではDocumentの編集権を手放せません",
        ))
    }
    fn edit(
        &self,
        id: &DocumentId,
        revision: Revision,
        range: Range<usize>,
        replacement: &str,
    ) -> Result<DocumentCache, DocumentError>;
    fn history(
        &self,
        id: &DocumentId,
        revision: Revision,
        undo: bool,
    ) -> Result<DocumentCache, DocumentError>;
    fn save(
        &self,
        id: &DocumentId,
        revision: Revision,
        path: Option<PathBuf>,
    ) -> Result<DocumentCache, DocumentError>;
    fn reload(&self, id: &DocumentId, revision: Revision) -> Result<DocumentCache, DocumentError>;
    fn discard(&self, id: &DocumentId, revision: Revision) -> Result<DocumentCache, DocumentError>;
    fn close(&self, id: &DocumentId) -> Result<(), DocumentError>;
}
