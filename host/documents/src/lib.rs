//! Host が保持する Document 資源の境界。

use std::collections::HashSet;

use client_api::{
    DocumentEncoding, DocumentId, DocumentSnapshot, Revision, SessionId, WorkspaceId,
    WorkspaceRelativePath,
};
use document::{Document, Encoding};

/// 接続 session が開いている Document と、その未保存状態。
pub struct DocumentResource {
    pub document_id: DocumentId,
    pub path: Option<WorkspaceRelativePath>,
    pub document: Document,
    pub attached_sessions: HashSet<SessionId>,
}

impl DocumentResource {
    pub fn new(document_id: DocumentId, path: WorkspaceRelativePath, document: Document) -> Self {
        Self {
            document_id,
            path: Some(path),
            document,
            attached_sessions: HashSet::new(),
        }
    }

    pub fn revision(&self) -> Revision {
        Revision::new(self.document.revision().number())
    }

    pub fn snapshot(&self, workspace_id: &WorkspaceId) -> DocumentSnapshot {
        DocumentSnapshot {
            document_id: self.document_id.clone(),
            workspace_id: workspace_id.clone(),
            path: self.path.clone(),
            content: self.document.content_for_save(),
            encoding: from_document_encoding(self.document.encoding()),
            revision: self.revision(),
            dirty: self.document.is_dirty(),
            can_undo: self.document.can_undo(),
            can_redo: self.document.can_redo(),
            external_change: client_api::DocumentExternalChange::Unchanged,
        }
    }
}

pub fn to_document_encoding(encoding: DocumentEncoding) -> Encoding {
    match encoding {
        DocumentEncoding::Utf8 => Encoding::Utf8,
        DocumentEncoding::Utf8Bom => Encoding::Utf8Bom,
        DocumentEncoding::Utf16Le => Encoding::Utf16Le,
        DocumentEncoding::Utf16Be => Encoding::Utf16Be,
    }
}

pub fn from_document_encoding(encoding: Encoding) -> DocumentEncoding {
    match encoding {
        Encoding::Utf8 => DocumentEncoding::Utf8,
        Encoding::Utf8Bom => DocumentEncoding::Utf8Bom,
        Encoding::Utf16Le => DocumentEncoding::Utf16Le,
        Encoding::Utf16Be => DocumentEncoding::Utf16Be,
    }
}
