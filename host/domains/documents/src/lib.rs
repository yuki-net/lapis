//! Host が保持する Document 資源の境界。

use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fmt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use client_api::{
    DocumentId, DocumentSnapshot, Revision, SessionId, WorkspaceId, WorkspaceRelativePath,
};
use document::Document;

/// Document 資源の参照・attach で発生するエラー。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentStateError {
    /// Document が開かれていない。
    NotFound,
    /// Document は開かれているが、この session に attach されていない。
    NotAttached,
}

impl fmt::Display for DocumentStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("document was not found"),
            Self::NotAttached => formatter.write_str("document is not attached to session"),
        }
    }
}

impl Error for DocumentStateError {}

/// Workspace が開いている Document の集合。
pub struct DocumentRegistry {
    resources: HashMap<DocumentId, DocumentResource>,
    by_path: HashMap<PathBuf, DocumentId>,
    /// 編集権を自動解放するまでの無操作時間。
    owner_idle_timeout: Duration,
}

impl Default for DocumentRegistry {
    fn default() -> Self {
        Self {
            resources: HashMap::new(),
            by_path: HashMap::new(),
            owner_idle_timeout: Duration::from_secs(300),
        }
    }
}

impl DocumentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 編集権の無操作タイムアウトを変更する（テスト・設定用）。
    pub fn with_owner_idle_timeout(mut self, timeout: Duration) -> Self {
        self.owner_idle_timeout = timeout;
        self
    }

    /// 無操作の編集権を解放し、解放した Document を返す。
    pub fn poll_owners(&mut self) -> Vec<DocumentId> {
        let idle_timeout = self.owner_idle_timeout;
        self.resources
            .values_mut()
            .filter(|resource| {
                resource.owner.is_some() && resource.last_activity.elapsed() >= idle_timeout
            })
            .map(|resource| {
                resource.owner = None;
                resource.document_id.clone()
            })
            .collect()
    }

    /// 指定 Session が持つ編集権をすべて解放し、解放した Document を返す。
    pub fn release_owned_by(&mut self, session_id: &SessionId) -> Vec<DocumentId> {
        self.resources
            .values_mut()
            .filter(|resource| resource.owner.as_ref() == Some(session_id))
            .map(|resource| {
                resource.owner = None;
                resource.document_id.clone()
            })
            .collect()
    }

    pub fn values(&self) -> impl Iterator<Item = &DocumentResource> {
        self.resources.values()
    }

    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut DocumentResource> {
        self.resources.values_mut()
    }

    pub fn get(&self, id: &DocumentId) -> Option<&DocumentResource> {
        self.resources.get(id)
    }

    pub fn get_mut(&mut self, id: &DocumentId) -> Option<&mut DocumentResource> {
        self.resources.get_mut(id)
    }

    pub fn insert(&mut self, id: DocumentId, resource: DocumentResource) {
        self.resources.insert(id, resource);
    }

    pub fn remove(&mut self, id: &DocumentId) -> Option<DocumentResource> {
        self.resources.remove(id)
    }

    pub fn is_empty(&self) -> bool {
        self.resources.is_empty()
    }

    pub fn contains_path(&self, path: &Path) -> bool {
        self.by_path.contains_key(path)
    }

    pub fn get_by_path(&self, path: &Path) -> Option<&DocumentId> {
        self.by_path.get(path)
    }

    pub fn insert_path(&mut self, path: PathBuf, id: DocumentId) {
        self.by_path.insert(path, id);
    }

    pub fn remove_path(&mut self, path: &Path) -> Option<DocumentId> {
        self.by_path.remove(path)
    }

    pub fn require_attached_mut(
        &mut self,
        session_id: &SessionId,
        document_id: &DocumentId,
    ) -> Result<&mut DocumentResource, DocumentStateError> {
        let document = self
            .resources
            .get_mut(document_id)
            .ok_or(DocumentStateError::NotFound)?;
        if !document.attached_sessions.contains(session_id) {
            return Err(DocumentStateError::NotAttached);
        }
        Ok(document)
    }
}

/// 接続 session が開いている Document と、その未保存状態。
pub struct DocumentResource {
    pub document_id: DocumentId,
    pub path: Option<WorkspaceRelativePath>,
    pub document: Document,
    pub attached_sessions: HashSet<SessionId>,
    /// 編集を許可する Session。`None` は未所有。
    pub owner: Option<SessionId>,
    /// 最後に編集権を行使した時刻。無操作で owner を解放する。
    pub last_activity: Instant,
}

impl DocumentResource {
    pub fn new(document_id: DocumentId, path: WorkspaceRelativePath, document: Document) -> Self {
        Self {
            document_id,
            path: Some(path),
            document,
            attached_sessions: HashSet::new(),
            owner: None,
            last_activity: Instant::now(),
        }
    }

    /// 編集権の活動時刻を更新する。
    pub fn touch(&mut self) {
        self.last_activity = Instant::now();
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
            encoding: self.document.encoding().into(),
            revision: self.revision(),
            dirty: self.document.is_dirty(),
            can_undo: self.document.can_undo(),
            can_redo: self.document.can_redo(),
            external_change: client_api::DocumentExternalChange::Unchanged,
            owner: self.owner.clone(),
        }
    }
}
