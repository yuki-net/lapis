use crate::{Document, DocumentError, ExternalChange, Revision};
use lapis_editor_core::DocumentId;
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
}

pub trait DocumentConnection: Send + Sync {
    fn fork(&self) -> std::sync::Arc<dyn DocumentConnection>;
    fn workspace(&self, root: &Path) -> Result<lapis_editor_core::WorkspaceId, DocumentError>;
    fn create(&self, root: Option<&Path>) -> Result<DocumentCache, DocumentError>;
    fn open(&self, path: &Path) -> Result<DocumentCache, DocumentError>;
    fn attach(&self, id: &DocumentId) -> Result<DocumentCache, DocumentError>;
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
    fn close(&self, id: &DocumentId) -> Result<(), DocumentError>;
}
