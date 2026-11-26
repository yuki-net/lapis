use super::*;
use document::{
    Document, DocumentCache, DocumentConnection, DocumentError, Encoding, ExternalChange, Position,
    Revision,
};
use editor_core::{DocumentId, WorkspaceId};
use std::{ops::Range, path::Path, path::PathBuf, sync::Arc};
use workspace::{FileEntry, WorkspaceError, WorkspaceIdentity};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentTab {
    pub id: DocumentId,
    pub display_name: String,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    pub external_change: ExternalChange,
    pub encoding: Encoding,
    pub active: bool,
}

struct OpenDocument {
    id: DocumentId,
    document: Document,
    external_change: ExternalChange,
    view: DocumentViewState,
    can_undo: bool,
    can_redo: bool,
    owner: Option<editor_core::SessionId>,
    is_owner: bool,
}

impl OpenDocument {
    fn from_cache(cache: DocumentCache, view: DocumentViewState) -> Self {
        Self {
            id: cache.id,
            document: cache.document,
            external_change: cache.external_change,
            can_undo: cache.can_undo,
            can_redo: cache.can_redo,
            owner: cache.owner,
            is_owner: cache.is_owner,
            view,
        }
    }

    fn update(&mut self, cache: DocumentCache) {
        self.document = cache.document;
        self.external_change = cache.external_change;
        self.can_undo = cache.can_undo;
        self.can_redo = cache.can_redo;
        self.owner = cache.owner;
        self.is_owner = cache.is_owner;
    }
}

/// 一括closeの確認済み進捗。永続化失敗でも閉鎖済みIDを保持する。
#[derive(Debug, Default)]
pub struct DocumentCloseProgress {
    pub closed: Vec<DocumentId>,
}

#[derive(Debug)]
pub struct DocumentCloseFailure {
    pub progress: DocumentCloseProgress,
    pub failed: Option<DocumentId>,
    pub remaining: Vec<DocumentId>,
    pub stage: DocumentCloseStage,
    pub error: DocumentError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentCloseStage {
    Validation,
    Close,
    Persistence,
}

impl std::fmt::Display for DocumentCloseFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let stage = match self.stage {
            DocumentCloseStage::Validation => "確認",
            DocumentCloseStage::Close => "閉鎖",
            DocumentCloseStage::Persistence => "保存",
        };
        let target = self.failed.as_ref().map_or("なし", |id| id.as_str());
        let closed = if self.progress.closed.is_empty() {
            "なし".to_string()
        } else {
            self.progress
                .closed
                .iter()
                .map(|id| id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        write!(
            f,
            "{stage}段階で失敗しました（対象: {target}、閉鎖済み: {closed}）: {}",
            self.error
        )
    }
}

impl std::error::Error for DocumentCloseFailure {}

/// Host の identity から Workspace の表示名を決める。
///
/// identity を取得できなかった場合は `None` を返す。Remote の root は Host の
/// 実ディレクトリではなく profile の label から作る仮想パスなので、取得失敗時に
/// root から名前を推測すると接続先と無関係な名前を表示してしまう。
fn workspace_display_name(
    identity: Result<Option<WorkspaceIdentity>, WorkspaceError>,
    root: &Path,
) -> Option<String> {
    match identity {
        Ok(Some(identity)) => Some(identity.display_name().trim().to_owned()),
        // identity を返さない接続先だけ root のディレクトリ名へ退避する。
        Ok(None) => Some(
            root.file_name()
                .and_then(|name| name.to_str())
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or_default()
                .to_owned(),
        ),
        Err(_) => None,
    }
}

/// UIからDocumentと外部I/Oを分離する、複数文書Workspace編集セッション。
pub struct EditorSession {
    documents: Vec<OpenDocument>,
    active: Option<usize>,
    workspace_root: Option<PathBuf>,
    workspace_restore_root: Option<PathBuf>,
    workspace_id: Option<WorkspaceId>,
    workspace_name: String,
    file_tree: Vec<FileEntry>,
    pub(crate) document_connection: Arc<dyn DocumentConnection>,
    pub(crate) repository: Arc<dyn WorkspaceRepository>,
    file_dialog: Arc<dyn WorkspaceDialog>,
    state_repository: Arc<dyn WorkspaceStateRepository>,
    restore_warning: Option<String>,
}

impl EditorSession {
    pub fn new(
        repository: Arc<dyn WorkspaceRepository>,
        file_dialog: Arc<dyn WorkspaceDialog>,
        state_repository: Arc<dyn WorkspaceStateRepository>,
        document_connection: Arc<dyn DocumentConnection>,
    ) -> Self {
        let mut session = Self::new_empty(
            repository,
            file_dialog,
            state_repository,
            document_connection,
        );
        if let Err(error) = session.restore() {
            session.restore_warning = Some(error.to_string());
        }
        if session.documents.is_empty() {
            session.new_document();
        }
        session
    }

    /// Creates a session for a new window without restoring a workspace or document.
    pub fn new_empty(
        repository: Arc<dyn WorkspaceRepository>,
        file_dialog: Arc<dyn WorkspaceDialog>,
        state_repository: Arc<dyn WorkspaceStateRepository>,
        document_connection: Arc<dyn DocumentConnection>,
    ) -> Self {
        Self {
            documents: Vec::new(),
            active: None,
            workspace_root: None,
            workspace_restore_root: None,
            workspace_id: None,
            workspace_name: "No Workspace".to_owned(),
            file_tree: Vec::new(),
            document_connection: document_connection.fork(),
            repository,
            file_dialog,
            state_repository,
            restore_warning: None,
        }
    }

    pub fn restore_warning(&self) -> Option<&str> {
        self.restore_warning.as_deref()
    }

    pub fn workspace_root(&self) -> Option<&Path> {
        self.workspace_root.as_deref()
    }

    pub fn workspace_path(
        &self,
        expected_root: &Path,
        input: &str,
    ) -> Result<PathBuf, WorkspaceError> {
        if self.workspace_root() != Some(expected_root) {
            return Err(WorkspaceError::new(
                "作業環境が切り替わりました。操作をやり直してください",
            ));
        }
        if input.is_empty()
            || Path::new(input)
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err(WorkspaceError::new(
                "Workspace内の相対pathを指定してください",
            ));
        }
        Ok(expected_root.join(input))
    }

    pub fn workspace_id(&self) -> Option<&WorkspaceId> {
        self.workspace_id.as_ref()
    }

    pub fn workspace_name(&self) -> &str {
        &self.workspace_name
    }

    pub fn snapshot(&self) -> WorkspaceViewState {
        WorkspaceViewState {
            root: self
                .workspace_root
                .clone()
                .or_else(|| self.workspace_restore_root.clone()),
            open_documents: self
                .documents
                .iter()
                .map(|open| DocumentViewState {
                    document_id: Some(open.id.clone()),
                    path: open.document.path().map(Path::to_owned).unwrap_or_default(),
                    cursor_char: open.view.cursor_char,
                    selection_start: open.view.selection_start,
                    selection_end: open.view.selection_end,
                    scroll_x: open.view.scroll_x,
                    scroll_y: open.view.scroll_y,
                    draft_content: open
                        .document
                        .is_dirty()
                        .then(|| open.document.content_for_save()),
                })
                .collect(),
            active_path: self
                .active_document()
                .and_then(|open| open.document.path().map(Path::to_owned)),
            active_document_id: self.active_document_id().cloned(),
        }
    }

    pub fn file_tree(&self) -> &[FileEntry] {
        &self.file_tree
    }

    pub fn tabs(&self) -> Vec<DocumentTab> {
        self.documents
            .iter()
            .enumerate()
            .map(|(index, open)| DocumentTab {
                id: open.id.clone(),
                display_name: if open.document.path().is_none() {
                    format!("Untitled {}", open.id.as_str())
                } else {
                    open.document.display_name()
                },
                path: open.document.path().map(Path::to_owned),
                dirty: open.document.is_dirty(),
                external_change: open.external_change,
                encoding: open.document.encoding(),
                active: self.active == Some(index),
            })
            .collect()
    }

    pub fn active_document_id(&self) -> Option<&DocumentId> {
        self.active_document().map(|open| &open.id)
    }

    pub fn active_path(&self) -> Option<&Path> {
        self.active_document()?.document.path()
    }

    pub fn active_revision(&self) -> Revision {
        self.active_document()
            .map(|open| open.document.revision())
            .unwrap_or_default()
    }

    pub fn active_text(&self) -> Result<String, DocumentError> {
        let document = &self.required_document()?.document;
        document.slice_chars(0..document.len_chars())
    }

    pub fn display_name(&self) -> String {
        self.active_document()
            .map(|open| open.document.display_name())
            .unwrap_or_else(|| "Untitled".to_owned())
    }

    pub fn revision(&self) -> u64 {
        self.active_document()
            .map(|open| open.document.revision().number())
            .unwrap_or_default()
    }

    pub fn encoding(&self) -> Encoding {
        self.active_document()
            .map(|open| open.document.encoding())
            .unwrap_or_default()
    }

    pub fn is_dirty(&self) -> bool {
        self.active_document()
            .is_some_and(|open| open.document.is_dirty())
    }

    /// 共有の要否を判断する、開いている文書とアクティブ文書の署名。
    ///
    /// 共有するのはタブ構成とアクティブ文書だけで、カーソル移動では変化しない。
    pub fn tab_signature(&self) -> (Vec<DocumentId>, Option<DocumentId>) {
        (
            self.documents.iter().map(|open| open.id.clone()).collect(),
            self.active_document_id().cloned(),
        )
    }

    pub fn external_change(&self) -> ExternalChange {
        self.active_document()
            .map(|open| open.external_change)
            .unwrap_or(ExternalChange::Unchanged)
    }

    pub fn has_external_change(&self) -> bool {
        self.external_change() != ExternalChange::Unchanged
    }

    /// 編集権を持つ Session。`None` は未所有。
    pub fn active_owner(&self) -> Option<&editor_core::SessionId> {
        self.active_document()?.owner.as_ref()
    }

    /// この接続が編集権を持つか。
    pub fn active_is_owner(&self) -> bool {
        self.active_document().is_some_and(|open| open.is_owner)
    }

    /// 他の Session が編集権を持ち、編集できない状態か。
    pub fn active_is_read_only(&self) -> bool {
        self.active_document()
            .is_some_and(|open| open.owner.is_some() && !open.is_owner)
    }

    /// 編集権を取得する。他の Session が所有中なら奪う。
    pub fn claim_active(&mut self) -> Result<(), DocumentError> {
        let id = self
            .active_document()
            .map(|open| open.id.clone())
            .ok_or_else(|| DocumentError::io("文書が開かれていません"))?;
        let cache = self.document_connection.claim(&id)?;
        self.required_document_mut()?.update(cache);
        Ok(())
    }

    /// 編集権を手放す。
    pub fn release_active(&mut self) -> Result<(), DocumentError> {
        let id = self
            .active_document()
            .map(|open| open.id.clone())
            .ok_or_else(|| DocumentError::io("文書が開かれていません"))?;
        let cache = self.document_connection.release(&id)?;
        self.required_document_mut()?.update(cache);
        Ok(())
    }

    pub fn len_chars(&self) -> usize {
        self.active_document()
            .map(|open| open.document.len_chars())
            .unwrap_or_default()
    }

    pub fn len_bytes(&self) -> usize {
        self.active_document()
            .map(|open| open.document.len_bytes())
            .unwrap_or_default()
    }

    pub fn len_lines(&self) -> usize {
        self.active_document()
            .map(|open| open.document.len_lines())
            .unwrap_or(1)
    }

    pub fn is_empty(&self) -> bool {
        self.active_document()
            .is_none_or(|open| open.document.is_empty())
    }

    pub fn line(&self, line: usize) -> Option<String> {
        self.active_document()?.document.line(line)
    }

    pub fn line_start_char(&self, line: usize) -> Result<usize, DocumentError> {
        self.required_document()?
            .document
            .position_to_char(Position::new(
                u32::try_from(line).map_err(|_| {
                    DocumentError::new(
                        document::DocumentErrorKind::InvalidRange,
                        "line index is too large",
                    )
                })?,
                0,
            ))
    }

    pub fn slice_chars(&self, range: Range<usize>) -> Result<String, DocumentError> {
        self.required_document()?.document.slice_chars(range)
    }

    pub fn char_to_byte(&self, char_index: usize) -> Result<usize, DocumentError> {
        self.required_document()?.document.char_to_byte(char_index)
    }

    pub fn byte_to_char(&self, byte_index: usize) -> Result<usize, DocumentError> {
        self.required_document()?.document.byte_to_char(byte_index)
    }

    pub fn char_to_position(&self, char_index: usize) -> Result<Position, DocumentError> {
        self.required_document()?
            .document
            .char_to_position(char_index)
    }

    pub fn position_to_char(&self, position: Position) -> Result<usize, DocumentError> {
        self.required_document()?
            .document
            .position_to_char(position)
    }

    pub fn char_to_utf16_offset(&self, char_index: usize) -> Result<usize, DocumentError> {
        self.required_document()?
            .document
            .char_to_utf16_offset(char_index)
    }

    pub fn utf16_offset_to_char(&self, offset: usize) -> Result<usize, DocumentError> {
        self.required_document()?
            .document
            .utf16_offset_to_char(offset)
    }

    pub fn replace_range(
        &mut self,
        range: Range<usize>,
        replacement: &str,
    ) -> Result<bool, DocumentError> {
        let open = self.required_document()?;
        if range.is_empty() && replacement.is_empty() {
            return Ok(false);
        }
        let old_revision = open.document.revision();
        let result = self
            .document_connection
            .edit(&open.id, old_revision, range, replacement);
        let cache = self.refresh_after_error(result)?;
        let changed = cache.document.revision() != old_revision;
        self.required_document_mut()?.update(cache);
        if changed {
            self.persist_best_effort();
        }
        Ok(changed)
    }

    pub fn undo(&mut self) -> bool {
        self.change_history(true)
    }

    pub fn redo(&mut self) -> bool {
        self.change_history(false)
    }

    pub fn can_undo(&self) -> bool {
        self.active_document().is_some_and(|open| open.can_undo)
    }

    pub fn can_redo(&self) -> bool {
        self.active_document().is_some_and(|open| open.can_redo)
    }

    pub fn find(&self, query: &str, case_sensitive: bool) -> Vec<Range<usize>> {
        self.active_document()
            .map(|open| open.document.find(query, case_sensitive))
            .unwrap_or_default()
    }

    pub fn active_view(&self) -> Option<&DocumentViewState> {
        self.active_document().map(|open| &open.view)
    }

    pub fn update_active_view(
        &mut self,
        selection: Range<usize>,
        cursor: usize,
        scroll_x: f32,
        scroll_y: f32,
    ) {
        if let Some(open) = self.active_document_mut() {
            open.view.selection_start = selection.start;
            open.view.selection_end = selection.end;
            open.view.cursor_char = cursor;
            open.view.scroll_x = scroll_x;
            open.view.scroll_y = scroll_y;
        }
        self.persist_best_effort();
    }

    pub fn new_document(&mut self) {
        let cache = match self.document_connection.create(self.workspace_root()) {
            Ok(cache) => cache,
            Err(error) => {
                self.restore_warning = Some(error.to_string());
                return;
            }
        };
        let id = cache.id.clone();
        let view = DocumentViewState {
            document_id: Some(id.clone()),
            ..DocumentViewState::default()
        };
        self.documents.push(OpenDocument::from_cache(cache, view));
        self.active = Some(self.documents.len() - 1);
        self.persist_best_effort();
    }

    pub fn choose_workspace(&mut self) -> Result<DocumentAction, WorkspaceError> {
        let Some(root) = self.file_dialog.choose_workspace_path() else {
            return Ok(DocumentAction::Cancelled);
        };
        self.open_workspace(root)?;
        Ok(DocumentAction::Completed)
    }

    pub fn choose_file(&mut self) -> Result<DocumentAction, DocumentError> {
        let Some(path) = self.file_dialog.choose_file_path() else {
            return Ok(DocumentAction::Cancelled);
        };
        self.open_path(path)?;
        Ok(DocumentAction::Completed)
    }

    pub fn open_workspace(&mut self, root: PathBuf) -> Result<(), WorkspaceError> {
        let workspace_id = self
            .document_connection
            .workspace(&root)
            .map_err(|error| WorkspaceError::new(error.to_string()))?;
        let tree = self.repository.list_tree(&root)?;
        let identity = self.repository.identity(&root);
        self.detach_documents();
        if self.workspace_root.as_deref() != Some(root.as_path()) {
            // 別の Workspace へ移るときは前の名前を引き継がない。
            self.workspace_name.clear();
        }
        if let Some(name) = workspace_display_name(identity, &root) {
            self.workspace_name = name;
        }
        self.workspace_root = Some(root);
        self.workspace_restore_root = None;
        self.workspace_id = Some(workspace_id);
        self.file_tree = tree;
        self.documents.clear();
        self.active = None;
        self.persist()?;
        Ok(())
    }

    pub fn close_workspace(&mut self) -> Result<(), WorkspaceError> {
        self.detach_documents();
        self.workspace_root = None;
        self.workspace_restore_root = None;
        self.workspace_id = None;
        self.workspace_name = "No Workspace".to_owned();
        self.file_tree.clear();
        self.documents.clear();
        self.active = None;
        self.persist()?;
        Ok(())
    }

    pub fn repository(&self) -> Arc<dyn WorkspaceRepository> {
        self.repository.clone()
    }

    /// 残すattachmentを再確立する。一部の取得失敗でも他のcache更新は保持する。
    pub fn refresh_documents(&mut self) -> Result<(), DocumentError> {
        let mut failure = None;
        for open in &mut self.documents {
            match self.document_connection.attach(&open.id) {
                Ok(cache) => open.update(cache),
                Err(error) => {
                    failure.get_or_insert(error);
                }
            }
        }
        failure.map_or(Ok(()), Err)
    }

    /// I/Oを行わず、Clientが最後に確認したattachmentと文書cacheを返す。
    pub fn document_caches(&self) -> Vec<DocumentCache> {
        self.documents
            .iter()
            .map(|open| DocumentCache {
                id: open.id.clone(),
                document: open.document.clone(),
                can_undo: open.can_undo,
                can_redo: open.can_redo,
                external_change: open.external_change,
                reused: true,
                owner: open.owner.clone(),
                is_owner: open.is_owner,
            })
            .collect()
    }

    /// 復旧情報の保存完了を終了barrierとして確認する。
    pub fn persist_documents(&self) -> Result<(), WorkspaceError> {
        self.persist()
    }

    pub fn document_connection(&self) -> Arc<dyn DocumentConnection> {
        self.document_connection.clone()
    }

    pub fn file_dialog(&self) -> Arc<dyn WorkspaceDialog> {
        self.file_dialog.clone()
    }

    pub fn state_repository(&self) -> Arc<dyn WorkspaceStateRepository> {
        self.state_repository.clone()
    }

    pub fn new_folder(&mut self) -> Result<(), WorkspaceError> {
        let root = self
            .workspace_root()
            .ok_or_else(|| WorkspaceError::new("Workspace を開いてください"))?;
        let timestamp = task::unix_time_ms();
        self.repository
            .create_directory(root, &format!("new-folder-{timestamp}"))?;
        self.refresh_file_tree()
    }

    pub fn refresh_file_tree(&mut self) -> Result<(), WorkspaceError> {
        let Some(root) = self.workspace_root.as_deref() else {
            return Ok(());
        };
        self.file_tree = self.repository.list_tree(root)?;
        Ok(())
    }

    pub fn open_path(&mut self, path: PathBuf) -> Result<(), DocumentError> {
        if let Some(index) = self
            .documents
            .iter()
            .position(|open| open.document.path() == Some(path.as_path()))
        {
            self.active = Some(index);
            self.persist_best_effort();
            return Ok(());
        }
        let cache = self.document_connection.open(&path)?;
        if let Some(index) = self.documents.iter().position(|open| open.id == cache.id) {
            self.documents[index].update(cache);
            self.active = Some(index);
            self.persist_best_effort();
            return Ok(());
        }
        let id = cache.id.clone();
        self.documents.push(OpenDocument::from_cache(
            cache,
            DocumentViewState {
                document_id: Some(id),
                path,
                ..DocumentViewState::default()
            },
        ));
        self.active = Some(self.documents.len() - 1);
        self.persist_best_effort();
        Ok(())
    }

    pub fn activate_document(&mut self, id: &DocumentId) -> bool {
        if !self.select_document_for_operation(id) {
            return false;
        }
        self.persist_best_effort();
        true
    }

    /// Host が共有した view を、開いているタブの構成とアクティブ文書だけへ反映する。
    ///
    /// 文書の内容・カーソル位置・スクロールは既に開いている文書では変更しない。
    /// 未保存の文書は共有側で閉じられていても破棄せず残す。適用できない文書
    /// （path を持たないuntitled など）は無視する。
    pub fn apply_shared_view(
        &mut self,
        view: &workbench::WorkspaceView,
    ) -> Result<(), WorkspaceError> {
        let Some(root) = self.workspace_root.clone() else {
            return Ok(());
        };
        if self.workspace_id.is_none() {
            return Ok(());
        }
        let mut slots: Vec<Option<OpenDocument>> = std::mem::take(&mut self.documents)
            .into_iter()
            .map(Some)
            .collect();
        let take = |slots: &mut Vec<Option<OpenDocument>>,
                    document_id: Option<&DocumentId>,
                    path: Option<&Path>| {
            slots.iter_mut().find_map(|slot| {
                let open = slot.as_ref()?;
                let matched = document_id.is_some_and(|id| &open.id == id)
                    || path.is_some_and(|path| open.document.path() == Some(path));
                matched.then(|| slot.take()).flatten()
            })
        };
        let mut documents = Vec::with_capacity(view.open_documents.len());
        for document in &view.open_documents {
            let path = document.path.as_ref().map(|path| root.join(path.as_str()));
            if let Some(open) = take(&mut slots, document.document_id.as_ref(), path.as_deref()) {
                documents.push(open);
                continue;
            }
            let Some(path) = path else {
                continue;
            };
            let Ok(cache) = self.document_connection.open(&path) else {
                continue;
            };
            let id = cache.id.clone();
            if let Some(open) = take(&mut slots, Some(&id), Some(&path)) {
                documents.push(open);
                continue;
            }
            documents.push(OpenDocument::from_cache(
                cache,
                DocumentViewState {
                    document_id: Some(id),
                    path,
                    ..DocumentViewState::default()
                },
            ));
        }
        let mut active = view
            .active_document_id
            .as_ref()
            .and_then(|id| documents.iter().position(|open| &open.id == id));
        if active.is_none() {
            // ID を共有しない Client（Web）のために、相対 path でも解決する。
            active = view.active_path.as_ref().and_then(|path| {
                let path = root.join(path.as_str());
                documents
                    .iter()
                    .position(|open| open.document.path() == Some(path.as_path()))
            });
        }
        if active.is_none() {
            active = self
                .active
                .and_then(|previous| slots.get(previous).and_then(Option::as_ref))
                .and_then(|open| documents.iter().position(|current| current.id == open.id));
        }
        // 未保存の文書は共有に含まれなくても破棄しない。保存されていない内容だけ残す。
        for slot in slots.into_iter().flatten() {
            if slot.document.is_dirty() {
                documents.push(slot);
            } else if let Err(error) = self.document_connection.close(&slot.id) {
                self.restore_warning = Some(error.to_string());
            }
        }
        if active.is_none() && !documents.is_empty() {
            active = Some(0);
        }
        self.documents = documents;
        self.active = active;
        self.persist_best_effort();
        Ok(())
    }

    /// Worker の操作対象だけを切り替える。表示状態の保存や文書 I/O は行わない。
    /// ユーザーによるタブ選択の保存には `activate_document` を使う。
    pub fn select_document_for_operation(&mut self, id: &DocumentId) -> bool {
        if self.active_document_id() == Some(id) {
            return true;
        }
        let Some(index) = self.documents.iter().position(|open| &open.id == id) else {
            return false;
        };
        self.active = Some(index);
        true
    }

    pub fn close_document(
        &mut self,
        id: &DocumentId,
        disposition: DocumentCloseDisposition,
    ) -> Result<bool, DocumentError> {
        let Some(index) = self.documents.iter().position(|open| &open.id == id) else {
            return Ok(false);
        };
        // 確認後の変更にDiscardの承認を流用しない。
        let current = self.document_connection.attach(id)?;
        let expected = self.documents[index].document.revision();
        self.documents[index].update(current);
        if self.documents[index].document.revision() != expected {
            self.persist_best_effort();
            return Err(DocumentError::conflict("確認後に文書が変更されました"));
        }
        if self.documents[index].document.is_dirty()
            && disposition == DocumentCloseDisposition::PreserveChanges
        {
            return Err(DocumentError::conflict(
                "未保存の文書は確認なしに閉じられません",
            ));
        }
        if disposition == DocumentCloseDisposition::DiscardChanges
            && self.documents[index].document.is_dirty()
        {
            match self
                .document_connection
                .discard(id, self.documents[index].document.revision())
            {
                Ok(cache) => self.documents[index].update(cache),
                Err(error) => {
                    if let Ok(cache) = self.document_connection.attach(id) {
                        self.documents[index].update(cache);
                        self.persist_best_effort();
                    }
                    return Err(error);
                }
            }
        }
        if let Err(error) = self.document_connection.close(id) {
            // 応答不明のcloseを成功と推測せず、残す意図で再attachする。
            if let Ok(cache) = self.document_connection.attach(id) {
                self.documents[index].update(cache);
            }
            return Err(error);
        }
        let was_active = self.active == Some(index);
        self.documents.remove(index);
        self.active = if self.documents.is_empty() {
            None
        } else if was_active {
            Some(index.min(self.documents.len() - 1))
        } else {
            self.active
                .map(|active| active.saturating_sub(usize::from(active > index)))
        };
        self.persist_best_effort();
        Ok(true)
    }

    /// 全判断を検証してから順番に閉じる。失敗時も確認済みcloseを返す。
    /// Close段階の失敗IDは応答不明を含む。再attach・再確認してから再試行する。
    pub fn close_documents(
        &mut self,
        documents: &[(DocumentId, DocumentCloseDisposition, document::Revision)],
    ) -> Result<DocumentCloseProgress, DocumentCloseFailure> {
        let mut progress = DocumentCloseProgress::default();
        for (index, decision) in documents.iter().enumerate() {
            if let Err(error) = self.validate_document_closes(std::slice::from_ref(decision)) {
                return Err(DocumentCloseFailure {
                    progress,
                    failed: Some(decision.0.clone()),
                    remaining: documents
                        .iter()
                        .enumerate()
                        .filter(|(other, _)| *other != index)
                        .map(|(_, (id, _, _))| id.clone())
                        .collect(),
                    stage: DocumentCloseStage::Validation,
                    error,
                });
            }
        }
        for (index, (id, disposition, revision)) in documents.iter().enumerate() {
            // 事前検証後の変更にもDiscardの承認を流用しない。
            let result = self
                .validate_document_closes(&[(id.clone(), *disposition, *revision)])
                .and_then(|()| self.close_document(id, *disposition));
            match result {
                Ok(true) => progress.closed.push(id.clone()),
                Ok(false) => {}
                Err(error) => {
                    return Err(DocumentCloseFailure {
                        progress,
                        failed: Some(id.clone()),
                        remaining: documents[index + 1..]
                            .iter()
                            .map(|(id, _, _)| id.clone())
                            .collect(),
                        stage: DocumentCloseStage::Close,
                        error,
                    });
                }
            }
        }
        if let Err(error) = self.persist_documents() {
            return Err(DocumentCloseFailure {
                progress,
                failed: None,
                remaining: Vec::new(),
                stage: DocumentCloseStage::Persistence,
                error: DocumentError::io(error.to_string()),
            });
        }
        Ok(progress)
    }

    /// Checks approved revisions and refreshes matching open-document caches without closing tabs.
    ///
    /// Each tuple supplies a document ID, approved disposition, and expected revision.
    /// Returns a conflict if a revision changed or a dirty document would be closed
    /// with `PreserveChanges`. Attachment errors propagate. Caches refreshed before
    /// an error remain updated; this method does not save or discard changes.
    pub fn validate_document_closes(
        &mut self,
        documents: &[(DocumentId, DocumentCloseDisposition, document::Revision)],
    ) -> Result<(), DocumentError> {
        for (id, disposition, revision) in documents {
            if !self.documents.iter().any(|open| &open.id == id) {
                return Err(DocumentError::conflict("対象文書は既に閉じられています"));
            }
            let current = self.document_connection.attach(id)?;
            if current.document.revision() != *revision {
                return Err(DocumentError::conflict("確認後に文書が変更されました"));
            }
            if current.document.is_dirty()
                && *disposition == DocumentCloseDisposition::PreserveChanges
            {
                return Err(DocumentError::conflict("未保存の文書を保持します"));
            }
            if let Some(open) = self.documents.iter_mut().find(|open| &open.id == id) {
                open.update(current);
            }
        }
        Ok(())
    }

    pub fn save_document(&mut self) -> Result<DocumentAction, DocumentError> {
        let path = if let Some(path) = self.required_document()?.document.path() {
            path.to_owned()
        } else {
            let Some(path) = self.file_dialog.choose_save_path(&self.display_name()) else {
                return Ok(DocumentAction::Cancelled);
            };
            path
        };

        let open = self.required_document()?;
        let target = if open.document.path().is_none() {
            Some(path)
        } else {
            None
        };
        let result = self
            .document_connection
            .save(&open.id, open.document.revision(), target);
        let cache = self.refresh_after_error(result)?;
        self.required_document_mut()?.update(cache);
        self.persist_best_effort();
        Ok(DocumentAction::Completed)
    }

    pub fn save_document_to(&mut self, path: PathBuf) -> Result<(), DocumentError> {
        let open = self.required_document()?;
        let result = self
            .document_connection
            .save(&open.id, open.document.revision(), Some(path));
        let cache = self.refresh_after_error(result)?;
        self.required_document_mut()?.update(cache);
        self.persist_best_effort();
        Ok(())
    }

    pub fn save_document_as(&mut self) -> Result<DocumentAction, DocumentError> {
        let Some(path) = self.file_dialog.choose_save_path(&self.display_name()) else {
            return Ok(DocumentAction::Cancelled);
        };
        self.save_document_to(path)?;
        Ok(DocumentAction::Completed)
    }

    pub fn reload_active_from_disk(&mut self) -> Result<(), DocumentError> {
        let open = self.required_document()?;
        let result = self
            .document_connection
            .reload(&open.id, open.document.revision());
        let cache = self.refresh_after_error(result)?;
        self.required_document_mut()?.update(cache);
        self.persist_best_effort();
        Ok(())
    }

    pub fn poll_external_changes(&mut self) -> Vec<(DocumentId, ExternalChange)> {
        let mut changes = Vec::new();
        for open in &mut self.documents {
            let cache = match self.document_connection.attach(&open.id) {
                Ok(value) => value,
                Err(_) => continue,
            };
            if cache.external_change != open.external_change
                || cache.document != open.document
                || cache.can_undo != open.can_undo
                || cache.can_redo != open.can_redo
            {
                changes.push((open.id.clone(), cache.external_change));
            }
            open.update(cache);
        }
        changes
    }

    fn required_document(&self) -> Result<&OpenDocument, DocumentError> {
        self.active_document()
            .ok_or_else(|| DocumentError::io("開いている文書がありません"))
    }

    fn required_document_mut(&mut self) -> Result<&mut OpenDocument, DocumentError> {
        self.active_document_mut()
            .ok_or_else(|| DocumentError::io("開いている文書がありません"))
    }

    fn active_document(&self) -> Option<&OpenDocument> {
        self.active.and_then(|index| self.documents.get(index))
    }

    fn active_document_mut(&mut self) -> Option<&mut OpenDocument> {
        self.active.and_then(|index| self.documents.get_mut(index))
    }

    fn change_history(&mut self, undo: bool) -> bool {
        let Some(open) = self.active_document() else {
            return false;
        };
        let revision = open.document.revision();
        let result = self.document_connection.history(&open.id, revision, undo);
        match self.refresh_after_error(result) {
            Ok(cache) => {
                let changed = cache.document.revision() != revision;
                if let Some(open) = self.active_document_mut() {
                    open.update(cache);
                }
                self.persist_best_effort();
                changed
            }
            Err(error) => {
                self.restore_warning = Some(error.to_string());
                false
            }
        }
    }

    fn refresh_after_error(
        &mut self,
        result: Result<DocumentCache, DocumentError>,
    ) -> Result<DocumentCache, DocumentError> {
        if result.is_err()
            && let Some(open) = self.active_document()
            && let Ok(cache) = self.document_connection.attach(&open.id)
            && let Some(open) = self.active_document_mut()
        {
            open.update(cache);
        }
        result
    }

    fn restore(&mut self) -> Result<(), WorkspaceError> {
        let Some(snapshot) = self.state_repository.load()? else {
            return Ok(());
        };
        self.restore_snapshot(snapshot)
    }

    fn recover_document_draft(&self, draft: &str) -> Result<DocumentCache, WorkspaceError> {
        let cache = self
            .document_connection
            .create(self.workspace_root())
            .map_err(|error| WorkspaceError::new(error.to_string()))?;
        self.document_connection
            .edit(
                &cache.id,
                cache.document.revision(),
                0..cache.document.len_chars(),
                draft,
            )
            .map_err(|error| WorkspaceError::new(error.to_string()))
    }

    pub fn recover_draft(&mut self, draft: &str) -> Result<(), DocumentError> {
        let cache = self
            .recover_document_draft(draft)
            .map_err(|error| DocumentError::io(error.to_string()))?;
        let id = cache.id.clone();
        self.documents.push(OpenDocument::from_cache(
            cache,
            DocumentViewState {
                document_id: Some(id),
                ..Default::default()
            },
        ));
        self.active = Some(self.documents.len() - 1);
        // 文書復旧は成功している。表示状態の保存失敗は警告として保持する。
        self.persist_best_effort();
        Ok(())
    }

    pub fn restore_snapshot(&mut self, snapshot: WorkspaceViewState) -> Result<(), WorkspaceError> {
        self.workspace_restore_root = snapshot.root.clone();
        self.detach_documents();
        self.documents.clear();
        self.active = None;
        self.workspace_root = None;
        self.workspace_id = None;
        self.file_tree.clear();
        self.workspace_name = "No Workspace".to_owned();
        if let Some(root) = snapshot.root {
            let workspace = self
                .document_connection
                .workspace(&root)
                .map_err(|error| WorkspaceError::new(error.to_string()))
                .and_then(|id| {
                    let identity = self.repository.identity(&root);
                    self.repository
                        .list_tree(&root)
                        .map(|tree| (id, tree, identity))
                });
            match workspace {
                Ok((id, tree, identity)) => {
                    self.workspace_id = Some(id);
                    self.workspace_name =
                        workspace_display_name(identity, &root).unwrap_or_default();
                    self.file_tree = tree;
                    self.workspace_root = Some(root);
                }
                Err(error) => {
                    self.restore_warning =
                        Some(format!("Workspace を復元できませんでした: {error}"));
                }
            }
        }
        let mut restored_active = None;
        for view in snapshot.open_documents {
            let was_active = snapshot.active_document_id.is_some()
                && view.document_id == snapshot.active_document_id;
            let attached = view
                .document_id
                .as_ref()
                .and_then(|id| self.document_connection.attach(id).ok());
            let recovered = attached.is_some();
            let cache = attached.map(Ok).unwrap_or_else(|| {
                if view.path.as_os_str().is_empty() {
                    self.document_connection.create(self.workspace_root())
                } else {
                    self.document_connection.open(&view.path)
                }
            });
            let mut cache = match cache {
                Ok(cache) => cache,
                Err(error) => {
                    let Some(draft) = &view.draft_content else {
                        continue;
                    };
                    let cache = self.recover_document_draft(draft)?;
                    self.restore_warning = Some(format!(
                        "元の文書を開けないため復元 draft を未保存文書として開きました: {error}"
                    ));
                    cache
                }
            };
            if let Some(draft) = &view.draft_content
                && cache.document.content_for_save() != *draft
            {
                if recovered || cache.reused {
                    if recovered && was_active {
                        restored_active = Some(self.documents.len());
                    }
                    self.documents
                        .push(OpenDocument::from_cache(cache, view.clone()));
                    cache = self
                        .document_connection
                        .create(self.workspace_root())
                        .map_err(|error| WorkspaceError::new(error.to_string()))?;
                    self.restore_warning = Some(
                        "復元 draft は共有文書を上書きせず未保存文書として開きました".to_owned(),
                    );
                }
                match self.document_connection.edit(
                    &cache.id,
                    cache.document.revision(),
                    0..cache.document.len_chars(),
                    draft,
                ) {
                    Ok(updated) => cache = updated,
                    Err(error) => {
                        let current = self.document_connection.attach(&cache.id).ok();
                        cache = self.recover_document_draft(draft)?;
                        if let Some(current) = current {
                            self.documents
                                .push(OpenDocument::from_cache(current, view.clone()));
                        }
                        self.restore_warning = Some(format!(
                            "復元 draft の適用に失敗したため未保存文書として開きました: {error}"
                        ));
                    }
                }
            }
            let id = cache.id.clone();
            if was_active && restored_active.is_none() {
                restored_active = Some(self.documents.len());
            }
            self.documents.push(OpenDocument::from_cache(
                cache,
                DocumentViewState {
                    document_id: Some(id),
                    ..view
                },
            ));
        }
        self.active = restored_active.or_else(|| {
            snapshot.active_path.and_then(|path| {
                self.documents
                    .iter()
                    .position(|open| open.document.path() == Some(path.as_path()))
            })
        });
        if self.active.is_none() && !self.documents.is_empty() {
            self.active = Some(0);
        }
        Ok(())
    }

    fn persist_best_effort(&mut self) {
        if let Err(error) = self.persist() {
            self.restore_warning = Some(error.to_string());
        }
    }

    fn detach_documents(&mut self) {
        for open in &self.documents {
            if let Err(error) = self.document_connection.close(&open.id) {
                self.restore_warning = Some(error.to_string());
            }
        }
    }

    pub(crate) fn persist(&self) -> Result<(), WorkspaceError> {
        self.state_repository.save(&self.snapshot())
    }
}
