use super::*;
use desktop_client::{
    ConversationRepository, WorkspaceDialog, WorkspaceRepository, WorkspaceSnapshot,
    WorkspaceStateRepository,
};
use document::{Document, DocumentConnection, DocumentError, ExternalChange, Revision};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};
use workspace::WorkspaceError;

#[derive(Default)]
pub(crate) struct State {
    pub documents: HashMap<DocumentId, Document>,
    pub attached: HashSet<DocumentId>,
    pub closed: Vec<DocumentId>,
    pub fail_close: Option<DocumentId>,
    pub fail_attach: bool,
    pub fail_attach_after_close: bool,
    pub fail_persist: bool,
}

#[test]
fn terminal_persistence_rejects_other_task_and_pending_context_without_erasing_resources() {
    use terminal::{TerminalId, TerminalSize, TerminalSnapshot, TerminalStatus};
    let fixture = Fixture::default();
    let mut client = fixture.client();
    let owner = client.conversation.active_id().clone();
    let terminal = TerminalSnapshot::empty(
        TerminalId::new("terminal-a"),
        PathBuf::from("task-a"),
        TerminalStatus::Running,
        TerminalSize::new(80, 24),
    );
    let mut view = Fixture::view();
    view.terminal_owner = Some(owner.clone());
    view.terminals = vec![terminal.clone()];
    client.persist_view(view).unwrap();
    for invalid_owner in [None, Some(ConversationId::new("task-b"))] {
        let mut view = Fixture::view();
        view.terminal_owner = invalid_owner;
        client.persist_view(view).unwrap();
        let record = client.conversation.active_record().unwrap();
        assert_eq!(record.terminals.len(), 1);
        assert_eq!(record.terminals[0].id.as_ref(), Some(&terminal.id));
    }
    let mut view = Fixture::view();
    view.terminal_owner = Some(owner);
    client.persist_view(view).unwrap();
    assert!(
        client
            .conversation
            .active_record()
            .unwrap()
            .terminals
            .is_empty()
    );
}

#[derive(Clone, Default)]
pub(crate) struct Fixture(pub Arc<Mutex<State>>);
impl Fixture {
    fn cache(id: &DocumentId, document: &Document) -> DocumentCache {
        DocumentCache {
            id: id.clone(),
            document: document.clone(),
            can_undo: document.can_undo(),
            can_redo: document.can_redo(),
            external_change: ExternalChange::Unchanged,
            reused: true,
            owner: None,
            is_owner: false,
        }
    }
    pub fn client(&self) -> DocumentClient {
        let port = Arc::new(self.clone());
        let mut editor =
            EditorSession::new_empty(port.clone(), port.clone(), port.clone(), port.clone());
        for _ in 0..3 {
            editor.new_document();
        }
        let conversation = ConversationSession::new(port.clone(), editor.snapshot());
        DocumentClient {
            editor,
            conversation,
            lsp: LspSession::new(port.clone()),

            edit_chain: EditChain::default(),
            git: port,
            snapshot_sequence: 0,
            document_owners: HashMap::new(),
        }
    }
    pub fn view() -> PersistedView {
        PersistedView {
            document: None,
            layout: Default::default(),
            execution: None,
            terminals: vec![],
            terminal_owner: None,
            input: None,
        }
    }
}
impl DocumentConnection for Fixture {
    fn fork(&self) -> Arc<dyn DocumentConnection> {
        Arc::new(self.clone())
    }
    fn workspace(&self, _: &Path) -> Result<WorkspaceId, DocumentError> {
        Ok(WorkspaceId::new("workspace"))
    }
    fn create(&self, _: Option<&Path>) -> Result<DocumentCache, DocumentError> {
        let mut state = self.0.lock().unwrap();
        let id = DocumentId::new(format!("doc-{}", state.documents.len()));
        let document = Document::new();
        let cache = Self::cache(&id, &document);
        state.documents.insert(id.clone(), document);
        state.attached.insert(id);
        Ok(cache)
    }
    fn open(&self, _: &Path) -> Result<DocumentCache, DocumentError> {
        unimplemented!()
    }
    fn attach(&self, id: &DocumentId) -> Result<DocumentCache, DocumentError> {
        let mut state = self.0.lock().unwrap();
        if state.fail_attach {
            return Err(DocumentError::io("attachment unavailable"));
        }
        let cache = Self::cache(id, state.documents.get(id).unwrap());
        state.attached.insert(id.clone());
        Ok(cache)
    }
    fn edit(
        &self,
        id: &DocumentId,
        revision: Revision,
        range: std::ops::Range<usize>,
        text: &str,
    ) -> Result<DocumentCache, DocumentError> {
        let mut state = self.0.lock().unwrap();
        let document = state.documents.get_mut(id).unwrap();
        assert_eq!(document.revision(), revision);
        document.replace_char_range(range, text)?;
        Ok(Self::cache(id, document))
    }
    fn history(
        &self,
        _: &DocumentId,
        _: Revision,
        _: bool,
    ) -> Result<DocumentCache, DocumentError> {
        unimplemented!()
    }
    fn save(
        &self,
        _: &DocumentId,
        _: Revision,
        _: Option<PathBuf>,
    ) -> Result<DocumentCache, DocumentError> {
        Err(DocumentError::io("save unavailable"))
    }
    fn reload(&self, _: &DocumentId, _: Revision) -> Result<DocumentCache, DocumentError> {
        unimplemented!()
    }
    fn discard(&self, id: &DocumentId, revision: Revision) -> Result<DocumentCache, DocumentError> {
        let mut state = self.0.lock().unwrap();
        let document = state.documents.get_mut(id).unwrap();
        assert_eq!(document.revision(), revision);
        document.discard(None)?;
        Ok(Self::cache(id, document))
    }
    fn close(&self, id: &DocumentId) -> Result<(), DocumentError> {
        let mut state = self.0.lock().unwrap();
        if state.fail_close.as_ref() == Some(id) {
            return Err(DocumentError::io("close unavailable"));
        }
        state.attached.remove(id);
        state.closed.push(id.clone());
        state.fail_attach |= state.fail_attach_after_close;
        Ok(())
    }
}
impl WorkspaceRepository for Fixture {
    fn create_directory(&self, _: &Path, _: &str) -> Result<(), WorkspaceError> {
        unimplemented!()
    }
    fn list_tree(&self, _: &Path) -> Result<Vec<FileEntry>, WorkspaceError> {
        Ok(vec![])
    }
}
impl WorkspaceDialog for Fixture {
    fn choose_workspace_path(&self) -> Option<PathBuf> {
        None
    }
    fn choose_save_path(&self, _: &str) -> Option<PathBuf> {
        None
    }
}
impl WorkspaceStateRepository for Fixture {
    fn load(&self) -> Result<Option<WorkspaceSnapshot>, WorkspaceError> {
        Ok(None)
    }
    fn save(&self, _: &WorkspaceSnapshot) -> Result<(), WorkspaceError> {
        Ok(())
    }
}
impl ConversationRepository for Fixture {
    fn load(&self) -> Result<desktop_client::ConversationState, WorkspaceError> {
        Ok(Default::default())
    }
    fn save(
        &self,
        _: &desktop_client::ConversationState,
        _: &[desktop_client::Project],
        _: &[ConversationRecord],
        _: &ConversationId,
    ) -> Result<(), WorkspaceError> {
        if self.0.lock().unwrap().fail_persist {
            Err(WorkspaceError::new("view disk full"))
        } else {
            Ok(())
        }
    }
}
impl git::GitConnection for Fixture {
    fn status(
        &self,
        _: &WorkspaceId,
        _: Option<&editor_core::TaskId>,
    ) -> Result<git::GitSnapshot, git::GitError> {
        Err(git::GitError::new(git::GitErrorKind::Io, "git unavailable"))
    }

    fn diff(
        &self,
        _: &WorkspaceId,
        _: Option<&editor_core::TaskId>,
        _: &str,
    ) -> Result<git::GitDiffSnapshot, git::GitError> {
        Err(git::GitError::new(git::GitErrorKind::Io, "git unavailable"))
    }

    fn worktrees(&self, _: &WorkspaceId) -> Result<Vec<git::WorktreeSnapshot>, git::GitError> {
        Err(git::GitError::new(git::GitErrorKind::Io, "git unavailable"))
    }

    fn create_worktree(&self, _: &WorkspaceId) -> Result<git::WorktreeSnapshot, git::GitError> {
        Err(git::GitError::new(git::GitErrorKind::Io, "git unavailable"))
    }

    fn import_file(
        &self,
        _: &WorkspaceId,
        _: &editor_core::TaskId,
        _: &str,
    ) -> Result<git::WorktreeSnapshot, git::GitError> {
        Err(git::GitError::new(git::GitErrorKind::Io, "git unavailable"))
    }

    fn discard_worktree(
        &self,
        _: &WorkspaceId,
        _: &editor_core::TaskId,
    ) -> Result<git::WorktreeSnapshot, git::GitError> {
        Err(git::GitError::new(git::GitErrorKind::Io, "git unavailable"))
    }
}
impl lsp::LanguageServerBackend for Fixture {
    fn start(&self, _: &Path) -> Result<(), lsp::LspError> {
        Ok(())
    }
    fn did_open(&self, _: &Path, _: &str, _: Revision) -> Result<(), lsp::LspError> {
        Ok(())
    }
    fn did_change(&self, _: &Path, _: &str, _: Revision) -> Result<(), lsp::LspError> {
        Ok(())
    }
    fn diagnostics(&self) -> Result<Vec<lsp::Diagnostic>, lsp::LspError> {
        Ok(vec![])
    }
    fn completion(
        &self,
        _: &Path,
        _: lsp::LspPosition,
        _: Revision,
    ) -> Result<Vec<lsp::CompletionItem>, lsp::LspError> {
        Ok(vec![])
    }
    fn definition(
        &self,
        _: &Path,
        _: lsp::LspPosition,
        _: Revision,
    ) -> Result<Option<lsp::DefinitionTarget>, lsp::LspError> {
        Ok(None)
    }
    fn shutdown(&self) -> Result<(), lsp::LspError> {
        Ok(())
    }
}

pub(crate) fn plan(
    client: &DocumentClient,
) -> Vec<(
    DocumentId,
    desktop_client::DocumentCloseDisposition,
    Revision,
)> {
    client
        .editor
        .document_caches()
        .into_iter()
        .map(|cache| {
            (
                cache.id,
                desktop_client::DocumentCloseDisposition::PreserveChanges,
                cache.document.revision(),
            )
        })
        .collect()
}

#[test]
fn restoring_a_remote_workbench_keeps_local_dirty_edits() {
    let fixture = Fixture::default();
    let mut client = fixture.client();
    client.editor.replace_range(0..0, "未保存の入力").unwrap();
    let shared = desktop_client::SharedWorkspaceView {
        workspace_id: WorkspaceId::new("workspace"),
        open_documents: Vec::new(),
        active_path: None,
        active_document_id: None,
    };
    let error = match client.restore_workbench(shared) {
        Ok(_) => panic!("未保存の変更があるのにHostの状態を適用しました"),
        Err(error) => error,
    };
    assert!(error.contains("未保存"), "予期しないエラー: {error}");
    assert_eq!(client.editor.active_text().unwrap(), "未保存の入力");
    assert!(client.editor.is_dirty());
}

#[test]
fn close_report_keeps_commits_when_view_persistence_or_snapshot_fails() {
    for (persistence_failure, snapshot_failure) in [(true, false), (false, true), (true, true)] {
        let fixture = Fixture::default();
        let mut client = fixture.client();
        let decisions = plan(&client);
        for (id, _, _) in &decisions {
            client.document_owners.insert(id.clone(), 1);
        }
        {
            let mut state = fixture.0.lock().unwrap();
            state.fail_persist = persistence_failure;
            state.fail_attach_after_close = snapshot_failure;
        }
        let report = client.close_documents(&decisions[..1], Fixture::view());
        assert_eq!(report.close.unwrap().closed, vec![decisions[0].0.clone()]);
        assert_eq!(report.persistence.is_err(), persistence_failure);
        assert_eq!(
            report.snapshot.synchronization_error.is_some(),
            snapshot_failure
        );
        assert_eq!(report.snapshot.tabs.len(), 2);
        assert!(
            !report
                .snapshot
                .document_owners
                .contains_key(&decisions[0].0)
        );
        assert!(!fixture.0.lock().unwrap().attached.contains(&decisions[0].0));
        {
            let mut state = fixture.0.lock().unwrap();
            state.fail_attach = false;
            state.fail_attach_after_close = false;
            state.fail_persist = false;
        }
        let fresh = client.snapshot().unwrap();
        assert!(fresh.sequence > report.snapshot.sequence);
        assert_eq!(fresh.tabs.len(), 2);
        assert!(
            client
                .close_documents(&plan(&client), Fixture::view())
                .close
                .is_ok()
        );
        assert!(client.snapshot().unwrap().tabs.is_empty());
        assert_eq!(fixture.0.lock().unwrap().closed.len(), 3);
    }
}

#[test]
fn lost_worker_reply_is_uncertain_even_when_one_close_committed() {
    let fixture = Fixture::default();
    let client = fixture.client();
    let decisions = plan(&client);
    let worker = ClientWorker::new(client);
    let receiver = worker
        .try_request(move |client| -> Result<(), String> {
            client.close_documents(&decisions[..1], Fixture::view());
            panic!("response lost after operation");
        })
        .unwrap();
    assert!(matches!(
        receiver.recv_timeout(Duration::from_secs(2)),
        Err(mpsc::RecvTimeoutError::Disconnected)
    ));
    // 返信senderの破棄とworker queueの破棄は別。unwind中に受理された要求も
    // 実行されず切断されることを待ってから、以降の受理拒否を確認する。
    if let Ok(reply) = worker.try_request(|client| client.snapshot()) {
        assert!(matches!(
            reply.recv_timeout(Duration::from_secs(2)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
    }
    assert!(worker.try_request(|client| client.snapshot()).is_err());
    let state = fixture.0.lock().unwrap();
    assert_eq!(state.closed.len(), 1);
    assert_eq!(state.attached.len(), 2);
    assert_eq!(state.documents.len(), 3);
}
