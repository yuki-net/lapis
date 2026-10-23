struct MemoryDocuments {
    repository: Arc<MemoryRepository>,
    documents: Arc<Mutex<HashMap<DocumentId, Document>>>,
    concurrent_edit: Arc<Mutex<Option<String>>>,
    workspace_failure: Arc<std::sync::atomic::AtomicBool>,
    faults: Arc<Mutex<CloseFaults>>,
}

#[derive(Default)]
struct CloseFaults {
    fail_close: Option<(DocumentId, bool)>,
    fail_attach: bool,
    fail_discard_response: bool,
    fail_save: bool,
    attached: std::collections::HashSet<DocumentId>,
    closed: Vec<DocumentId>,
    discarded: Vec<DocumentId>,
}

#[test]
fn internal_document_selection_and_idle_polls_do_not_persist_window_activity() {
    use std::sync::atomic::Ordering;
    let repository = Arc::new(MemoryRepository::default());
    let state = Arc::new(MemoryState::default());
    let mut editor = session(repository, state.clone());
    let root = editor.active_document_id().unwrap().clone();
    editor.new_document();
    let child = editor.active_document_id().unwrap().clone();
    let before = state.saves.load(Ordering::Relaxed);
    let persisted = state.load().unwrap();

    // 同じ対象の反復と、Root / 子の交互送信の両方で保存を増やさない。
    for id in [&root, &root, &child, &root, &child] {
        assert!(editor.select_document_for_operation(id));
        editor.poll_external_changes();
        assert_eq!(editor.active_document_id(), Some(id));
        assert_eq!(state.saves.load(Ordering::Relaxed), before);
        assert_eq!(state.load().unwrap(), persisted);
    }
    assert!(!editor.select_document_for_operation(&DocumentId::new("closed")));
    assert_eq!(editor.active_document_id(), Some(&child));
    assert_eq!(state.saves.load(Ordering::Relaxed), before);

    // 実際の選択・編集・表示状態更新の保存は維持する。
    assert!(editor.activate_document(&root));
    assert_eq!(state.saves.load(Ordering::Relaxed), before + 1);
    assert_eq!(state.load().unwrap().unwrap().active_document_id, Some(root.clone()));
    assert!(editor.select_document_for_operation(&child));
    editor.replace_range(0..0, "child draft").unwrap();
    assert_eq!(state.saves.load(Ordering::Relaxed), before + 2);
    assert_eq!(editor.active_text().unwrap(), "child draft");
    assert!(editor.select_document_for_operation(&root));
    assert!(editor.active_text().unwrap().is_empty());
    editor.update_active_view(0..0, 0, 0., 10.);
    assert_eq!(state.saves.load(Ordering::Relaxed), before + 3);
}

#[test]
fn close_failure_display_uses_readable_stage_and_document_ids() {
    let persistence_failure = DocumentCloseFailure {
        progress: DocumentCloseProgress {
            closed: vec![DocumentId::new("doc-a"), DocumentId::new("doc-b")],
        },
        failed: None,
        remaining: Vec::new(),
        stage: DocumentCloseStage::Persistence,
        error: DocumentError::io("view state failed"),
    };
    assert_eq!(
        persistence_failure.to_string(),
        "保存段階で失敗しました（対象: なし、閉鎖済み: doc-a, doc-b）: view state failed"
    );

    let close_failure = DocumentCloseFailure {
        progress: DocumentCloseProgress::default(),
        failed: Some(DocumentId::new("doc-c")),
        remaining: vec![DocumentId::new("doc-d")],
        stage: DocumentCloseStage::Close,
        error: DocumentError::io("close rejected"),
    };
    assert_eq!(
        close_failure.to_string(),
        "閉鎖段階で失敗しました（対象: doc-c、閉鎖済み: なし）: close rejected"
    );
}

#[test]
fn partial_close_reports_commits_and_retry_uses_remaining_attachments() {
    for failed_index in [0, 2] {
        let repository = Arc::new(MemoryRepository::default());
        let connection = Arc::new(MemoryDocuments::new(repository.clone()));
        let mut editor = EditorSession::new_empty(repository, Arc::new(FixedDialog {
            workspace: None, save: None,
        }), Arc::new(MemoryState::default()), connection.clone());
        for _ in 0..3 { editor.new_document(); }
        let ids = editor.tabs().iter().map(|tab| tab.id.clone()).collect::<Vec<_>>();
        let plan = ids.iter().map(|id| (id.clone(), DocumentCloseDisposition::PreserveChanges,
            connection.attach(id).unwrap().document.revision())).collect::<Vec<_>>();
        connection.faults.lock().unwrap().fail_close = Some((ids[failed_index].clone(), false));
        let failure = editor.close_documents(&plan).unwrap_err();
        assert_eq!(failure.stage, DocumentCloseStage::Close);
        assert_eq!(failure.progress.closed, ids[..failed_index]);
        assert_eq!(failure.failed.as_ref(), Some(&ids[failed_index]));
        assert_eq!(failure.remaining, ids[failed_index + 1..]);
        assert_eq!(editor.tabs().iter().map(|tab| tab.id.clone()).collect::<Vec<_>>(), ids[failed_index..]);
        let faults = connection.faults.lock().unwrap();
        for id in &ids[..failed_index] { assert!(!faults.attached.contains(id)); }
        for id in &ids[failed_index..] { assert!(faults.attached.contains(id)); }
        drop(faults);
        connection.faults.lock().unwrap().fail_close = None;
        editor.close_documents(&plan[failed_index..]).unwrap();
        assert!(editor.tabs().is_empty());
        assert!(connection.faults.lock().unwrap().attached.is_empty());
        assert_eq!(connection.faults.lock().unwrap().closed, ids);
        assert!(editor.close_documents(&plan).is_err());
        assert!(connection.faults.lock().unwrap().attached.is_empty());
        // closeは正本の削除ではない。
        assert_eq!(connection.documents.lock().unwrap().len(), 3);
    }
}

#[test]
fn close_commit_survives_persistence_and_snapshot_failures() {
    let repository = Arc::new(MemoryRepository::default());
    let state = Arc::new(MemoryState::default());
    let connection = Arc::new(MemoryDocuments::new(repository.clone()));
    let mut editor = EditorSession::new_empty(repository, Arc::new(FixedDialog {
        workspace: None, save: None,
    }), state.clone(), connection.clone());
    editor.new_document();
    let closed = editor.active_document_id().unwrap().clone();
    let revision = editor.active_revision();
    editor.new_document();
    let retained = editor.active_document_id().unwrap().clone();
    state.fail.store(true, std::sync::atomic::Ordering::SeqCst);
    let failure = editor.close_documents(&[(closed.clone(), DocumentCloseDisposition::PreserveChanges, revision)]).unwrap_err();
    assert_eq!(failure.stage, DocumentCloseStage::Persistence);
    assert_eq!(failure.progress.closed, vec![closed.clone()]);
    assert!(failure.failed.is_none());
    connection.faults.lock().unwrap().fail_attach = true;
    assert!(editor.refresh_documents().is_err());
    assert_eq!(editor.document_caches().iter().map(|cache| cache.id.clone()).collect::<Vec<_>>(), vec![retained.clone()]);
    assert!(!connection.faults.lock().unwrap().attached.contains(&closed));
    connection.faults.lock().unwrap().fail_attach = false;
    state.fail.store(false, std::sync::atomic::Ordering::SeqCst);
    editor.refresh_documents().unwrap();
    editor.persist_documents().unwrap();
    assert_eq!(state.load().unwrap().unwrap().open_documents[0].document_id, Some(retained));
    assert_eq!(connection.faults.lock().unwrap().closed, vec![closed]);
}

#[test]
fn discard_or_close_response_loss_reconciles_before_fresh_approval() {
    for discard_response_lost in [true, false] {
        let repository = Arc::new(MemoryRepository::default());
        let connection = Arc::new(MemoryDocuments::new(repository.clone()));
        let mut editor = EditorSession::new_empty(repository, Arc::new(FixedDialog {
            workspace: None, save: None,
        }), Arc::new(MemoryState::default()), connection.clone());
        editor.new_document();
        editor.replace_range(0..0, "approved draft").unwrap();
        let id = editor.active_document_id().unwrap().clone();
        let revision = editor.active_revision();
        {
            let mut faults = connection.faults.lock().unwrap();
            if discard_response_lost { faults.fail_discard_response = true; }
            else { faults.fail_close = Some((id.clone(), true)); }
        }
        let failure = editor.close_documents(&[(id.clone(), DocumentCloseDisposition::DiscardChanges, revision)]).unwrap_err();
        assert!(failure.progress.closed.is_empty());
        assert_eq!(failure.failed, Some(id.clone()));
        assert_eq!(editor.tabs().len(), 1);
        assert!(!editor.is_dirty());
        assert!(connection.faults.lock().unwrap().attached.contains(&id));
        editor.refresh_documents().unwrap();
        editor.close_documents(&[(id.clone(), DocumentCloseDisposition::PreserveChanges, editor.active_revision())]).unwrap();
        assert!(editor.tabs().is_empty());
        assert_eq!(connection.faults.lock().unwrap().discarded, vec![id]);
    }
}

#[test]
fn failed_save_retains_canonical_input_and_recovery_draft_then_retries() {
    let repository = Arc::new(MemoryRepository::default());
    let path = PathBuf::from("save-failure.md");
    insert_file(&repository, path.clone(), "base");
    let state = Arc::new(MemoryState::default());
    let connection = Arc::new(MemoryDocuments::new(repository.clone()));
    let mut editor = EditorSession::new_empty(repository.clone(), Arc::new(FixedDialog {
        workspace: None, save: None,
    }), state.clone(), connection.clone());
    editor.open_path(path.clone()).unwrap();
    editor.replace_range(0..4, "unsaved input").unwrap();
    let id = editor.active_document_id().unwrap().clone();
    connection.faults.lock().unwrap().fail_save = true;
    assert!(editor.save_document().is_err());
    assert!(editor.is_dirty());
    assert_eq!(editor.active_text().unwrap(), "unsaved input");
    assert_eq!(connection.attach(&id).unwrap().document.content_for_save(), "unsaved input");
    assert_eq!(state.load().unwrap().unwrap().open_documents[0].draft_content.as_deref(), Some("unsaved input"));
    assert!(editor.close_documents(&[(id.clone(), DocumentCloseDisposition::PreserveChanges, editor.active_revision())]).is_err());
    assert!(connection.faults.lock().unwrap().closed.is_empty());
    connection.faults.lock().unwrap().fail_save = false;
    assert_eq!(editor.save_document().unwrap(), DocumentAction::Completed);
    assert!(!editor.is_dirty());
    editor.close_documents(&[(id, DocumentCloseDisposition::PreserveChanges, editor.active_revision())]).unwrap();
    assert!(editor.tabs().is_empty());
    assert_eq!(repository.read_file(&path).unwrap().bytes, b"unsaved input");
}

#[test]
fn rejected_optimistic_input_can_be_recovered_without_overwriting_the_shared_document() {
    let repository = Arc::new(MemoryRepository::default());
    let path = PathBuf::from("shared.md");
    insert_file(&repository, path.clone(), "newer remote content");
    let documents = Arc::new(MemoryDocuments::new(repository.clone()));
    let state = Arc::new(MemoryState::default());
    let mut editor = EditorSession::new_empty(
        repository,
        Arc::new(FixedDialog {
            workspace: None,
            save: None,
        }),
        state.clone(),
        documents.clone(),
    );
    editor.open_path(path.clone()).unwrap();
    let original = editor.active_document_id().unwrap().clone();
    editor.recover_draft("all optimistic input 😀").unwrap();
    assert_ne!(editor.active_document_id(), Some(&original));
    assert!(editor.active_path().is_none());
    assert!(editor.is_dirty());
    assert_eq!(editor.active_text().unwrap(), "all optimistic input 😀");
    assert_eq!(
        documents
            .attach(&original)
            .unwrap()
            .document
            .content_for_save(),
        "newer remote content"
    );
    let persisted = state.load().unwrap().unwrap();
    assert_eq!(persisted.open_documents.len(), 2);
    assert_eq!(
        persisted.open_documents[1].draft_content.as_deref(),
        Some("all optimistic input 😀")
    );
    editor.restore_snapshot(persisted).unwrap();
    let recovered = editor
        .tabs()
        .into_iter()
        .find(|tab| tab.path.is_none())
        .unwrap();
    assert_eq!(editor.active_document_id(), Some(&recovered.id));
    assert_eq!(editor.active_text().unwrap(), "all optimistic input 😀");
}


#[test]
fn recovered_document_survives_view_state_persistence_failure() {
    struct FailingState;
    impl WorkspaceStateRepository for FailingState {
        fn load(&self) -> Result<Option<WorkspaceSnapshot>, WorkspaceError> {
            Ok(None)
        }
        fn save(&self, _: &WorkspaceSnapshot) -> Result<(), WorkspaceError> {
            Err(WorkspaceError::new("disk full"))
        }
    }

    let repository = Arc::new(MemoryRepository::default());
    let connection = Arc::new(MemoryDocuments::new(repository.clone()));
    let mut editor = EditorSession::new_empty(
        repository,
        Arc::new(FixedDialog {
            workspace: None,
            save: None,
        }),
        Arc::new(FailingState),
        connection.clone(),
    );
    editor.recover_draft("locally recovered input").unwrap();
    assert_eq!(editor.tabs().len(), 1);
    assert_eq!(editor.active_text().unwrap(), "locally recovered input");
    assert!(editor.is_dirty());
    assert!(editor.restore_warning().unwrap().contains("disk full"));
    let document = editor.active_document_id().unwrap();
    assert_eq!(
        connection.attach(document).unwrap().document.content_for_save(),
        "locally recovered input"
    );
}

impl MemoryDocuments {
    fn new(repository: Arc<MemoryRepository>) -> Self {
        Self {
            repository,
            documents: Arc::new(Mutex::new(HashMap::new())),
            concurrent_edit: Arc::new(Mutex::new(None)),
            workspace_failure: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            faults: Arc::new(Mutex::new(CloseFaults::default())),
        }
    }

    fn cache(&self, id: &DocumentId, document: &Document) -> DocumentCache {
        let actual = document
            .path()
            .and_then(|path| self.repository.fingerprint(path).ok().flatten());
        DocumentCache {
            reused: true,
            id: id.clone(),
            document: document.clone(),
            can_undo: document.can_undo(),
            can_redo: document.can_redo(),
            external_change: document.external_change(actual.as_ref()),
        }
    }

    fn mutate(
        &self,
        id: &DocumentId,
        revision: Revision,
        operation: impl FnOnce(&mut Document) -> Result<(), DocumentError>,
    ) -> Result<DocumentCache, DocumentError> {
        let mut documents = self.documents.lock().unwrap();
        let document = documents
            .get_mut(id)
            .ok_or_else(|| DocumentError::io("not found"))?;
        if document.revision() != revision {
            return Err(DocumentError::conflict("stale revision"));
        }
        operation(document)?;
        Ok(self.cache(id, document))
    }
}

impl DocumentConnection for MemoryDocuments {
    fn fork(&self) -> Arc<dyn DocumentConnection> {
        Arc::new(Self {
            repository: self.repository.clone(),
            documents: self.documents.clone(),
            concurrent_edit: self.concurrent_edit.clone(),
            workspace_failure: self.workspace_failure.clone(),
            faults: self.faults.clone(),
        })
    }
    fn workspace(&self, _: &Path) -> Result<WorkspaceId, DocumentError> {
        if self.workspace_failure.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(DocumentError::io("workspace temporarily unavailable"));
        }
        Ok(WorkspaceId::new("test-workspace"))
    }
    fn create(&self, _: Option<&Path>) -> Result<DocumentCache, DocumentError> {
        let mut documents = self.documents.lock().unwrap();
        let id = DocumentId::new(format!("test-document-{}", documents.len() + 1));
        let document = Document::new();
        let mut cache = self.cache(&id, &document);
        cache.reused = false;
        documents.insert(id, document);
        Ok(cache)
    }
    fn open(&self, path: &Path) -> Result<DocumentCache, DocumentError> {
        let mut documents = self.documents.lock().unwrap();
        if let Some((id, document)) = documents
            .iter()
            .find(|(_, document)| document.path() == Some(path))
        {
            return Ok(self.cache(id, document));
        }
        let document = Document::from_file(path.to_owned(), self.repository.read_file(path)?)?;
        let id = DocumentId::new(format!("test-document-{}", documents.len() + 1));
        let mut cache = self.cache(&id, &document);
        cache.reused = false;
        documents.insert(id, document);
        Ok(cache)
    }
    fn attach(&self, id: &DocumentId) -> Result<DocumentCache, DocumentError> {
        let mut faults = self.faults.lock().unwrap();
        if faults.fail_attach {
            return Err(DocumentError::io("attach unavailable"));
        }
        faults.attached.insert(id.clone());
        drop(faults);
        let documents = self.documents.lock().unwrap();
        Ok(self.cache(
            id,
            documents
                .get(id)
                .ok_or_else(|| DocumentError::io("not found"))?,
        ))
    }
    fn edit(
        &self,
        id: &DocumentId,
        revision: Revision,
        range: Range<usize>,
        replacement: &str,
    ) -> Result<DocumentCache, DocumentError> {
        if let Some(content) = self.concurrent_edit.lock().unwrap().take() {
            self.mutate(id, revision, |document| {
                document
                    .replace_char_range(0..document.len_chars(), &content)
                    .map(|_| ())
            })?;
        }
        self.mutate(id, revision, |document| {
            document.replace_char_range(range, replacement).map(|_| ())
        })
    }
    fn history(
        &self,
        id: &DocumentId,
        revision: Revision,
        undo: bool,
    ) -> Result<DocumentCache, DocumentError> {
        self.mutate(id, revision, |document| {
            if undo {
                document.undo();
            } else {
                document.redo();
            }
            Ok(())
        })
    }
    fn save(
        &self,
        id: &DocumentId,
        revision: Revision,
        path: Option<PathBuf>,
    ) -> Result<DocumentCache, DocumentError> {
        if self.faults.lock().unwrap().fail_save {
            return Err(DocumentError::io("save disk full"));
        }
        self.mutate(id, revision, |document| {
            let path = path
                .or_else(|| document.path().map(Path::to_owned))
                .ok_or_else(|| DocumentError::io("no path"))?;
            let fingerprint = self.repository.write_file(
                &path,
                &document.encoded_bytes(),
                document.saved_fingerprint(),
            )?;
            document.mark_saved(path, fingerprint);
            Ok(())
        })
    }
    fn reload(&self, id: &DocumentId, revision: Revision) -> Result<DocumentCache, DocumentError> {
        self.mutate(id, revision, |document| {
            if document.is_dirty() {
                return Err(DocumentError::conflict("dirty"));
            }
            document.reload(
                self.repository.read_file(
                    document
                        .path()
                        .ok_or_else(|| DocumentError::io("no path"))?,
                )?,
            )
        })
    }
    fn discard(&self, id: &DocumentId, revision: Revision) -> Result<DocumentCache, DocumentError> {
        let result = self.mutate(id, revision, |document| {
            let saved = if document.saved_fingerprint().is_some() {
                let path = document
                    .path()
                    .ok_or_else(|| DocumentError::conflict("no saved path"))?;
                Some(
                    self.repository
                        .read_file(path)
                        .map_err(|_| DocumentError::conflict("saved file unavailable"))?,
                )
            } else {
                None
            };
            document.discard(saved)
        })?;
        let mut faults = self.faults.lock().unwrap();
        faults.discarded.push(id.clone());
        if faults.fail_discard_response {
            faults.fail_discard_response = false;
            return Err(DocumentError::io("discard response lost"));
        }
        Ok(result)
    }

    fn close(&self, id: &DocumentId) -> Result<(), DocumentError> {
        let mut faults = self.faults.lock().unwrap();
        let failure = faults.fail_close.as_ref().filter(|(target, _)| target == id).cloned();
        if failure.as_ref().is_some_and(|(_, after)| !after) {
            return Err(DocumentError::io("close rejected"));
        }
        faults.attached.remove(id);
        faults.closed.push(id.clone());
        if failure.is_some() {
            faults.fail_close = None;
            return Err(DocumentError::io("close response lost"));
        }
        Ok(())
    }
}

#[test]
fn shared_clients_reject_stale_edits_and_refresh_canonical_history() {
    let repository = Arc::new(MemoryRepository::default());
    let path = PathBuf::from("shared.md");
    insert_file(&repository, path.clone(), "base");
    let documents = Arc::new(MemoryDocuments::new(repository.clone()));
    let make_editor = || {
        EditorSession::new_empty(
            repository.clone(),
            Arc::new(FixedDialog {
                workspace: None,
                save: None,
            }),
            Arc::new(MemoryState::default()),
            documents.clone(),
        )
    };
    let mut first = make_editor();
    let mut second = make_editor();
    first.open_path(path.clone()).unwrap();
    second.open_path(path).unwrap();
    assert_eq!(first.active_document_id(), second.active_document_id());
    first.replace_range(0..4, "shared😀").unwrap();
    assert_eq!(
        second.replace_range(0..4, "stale").unwrap_err().kind(),
        DocumentErrorKind::Conflict
    );
    assert_eq!(second.active_text().unwrap(), "shared😀");
    assert!(second.undo());
    first.poll_external_changes();
    assert_eq!(first.active_text().unwrap(), "base");
    assert!(first.redo());
    second.poll_external_changes();
    assert_eq!(second.active_text().unwrap(), "shared😀");
}

#[test]
fn discard_close_does_not_revive_dirty_content_on_reopen() {
    let repository = Arc::new(MemoryRepository::default());
    let path = PathBuf::from("discard.md");
    insert_file(&repository, path.clone(), "saved");
    let documents = Arc::new(MemoryDocuments::new(repository.clone()));
    let mut editor = EditorSession::new_empty(
        repository,
        Arc::new(FixedDialog {
            workspace: None,
            save: None,
        }),
        Arc::new(MemoryState::default()),
        documents.clone(),
    );
    editor.open_path(path.clone()).unwrap();
    let id = editor.active_document_id().unwrap().clone();
    editor.replace_range(0..5, "discarded draft").unwrap();
    let revision = editor.active_revision();
    assert!(
        editor
            .close_document(&id, DocumentCloseDisposition::DiscardChanges)
            .unwrap()
    );
    editor.open_path(path).unwrap();
    assert_eq!(editor.active_document_id(), Some(&id));
    assert_eq!(editor.active_text().unwrap(), "saved");
    assert!(editor.active_revision() > revision);
    assert!(!editor.is_dirty());
    assert!(!editor.can_undo());
    assert!(!documents.attach(&id).unwrap().document.is_dirty());
}

#[test]
fn stale_discard_close_keeps_tab_and_refreshes_newer_canonical_edits() {
    let repository = Arc::new(MemoryRepository::default());
    let path = PathBuf::from("discard.md");
    insert_file(&repository, path.clone(), "saved");
    let documents = Arc::new(MemoryDocuments::new(repository.clone()));
    let state = Arc::new(MemoryState::default());
    let mut editor = EditorSession::new_empty(
        repository,
        Arc::new(FixedDialog {
            workspace: None,
            save: None,
        }),
        state.clone(),
        documents.clone(),
    );
    editor.open_path(path).unwrap();
    let id = editor.active_document_id().unwrap().clone();
    let old_revision = editor.active_revision();
    let updated = documents
        .edit(&id, old_revision, 0..5, "other clients draft")
        .unwrap();
    assert_eq!(
        editor
            .close_document(&id, DocumentCloseDisposition::DiscardChanges)
            .unwrap_err()
            .kind(),
        DocumentErrorKind::Conflict
    );
    assert_eq!(editor.tabs().len(), 1);
    assert_eq!(editor.active_document_id(), Some(&id));
    assert_eq!(editor.active_text().unwrap(), "other clients draft");
    assert_eq!(editor.active_revision(), updated.document.revision());
    assert!(editor.is_dirty());
    assert_eq!(documents.attach(&id).unwrap().document, updated.document);
    let persisted = state.load().unwrap().unwrap();
    assert_eq!(persisted.open_documents.len(), 1);
    assert_eq!(persisted.open_documents[0].document_id.as_ref(), Some(&id));
    assert_eq!(
        persisted.open_documents[0].draft_content.as_deref(),
        Some("other clients draft")
    );
}

#[test]
fn restored_views_do_not_overwrite_live_canonical_content() {
    let repository = Arc::new(MemoryRepository::default());
    let documents = Arc::new(MemoryDocuments::new(repository.clone()));
    let mut editor = EditorSession::new_empty(
        repository,
        Arc::new(FixedDialog {
            workspace: None,
            save: None,
        }),
        Arc::new(MemoryState::default()),
        documents,
    );
    editor.new_document();
    editor.replace_range(0..0, "old").unwrap();
    let snapshot = editor.snapshot();
    editor.replace_range(0..3, "new").unwrap();
    editor.restore_snapshot(snapshot).unwrap();
    assert_eq!(editor.active_text().unwrap(), "new");
}

#[test]
fn persisted_draft_without_live_id_never_overwrites_an_existing_canonical_resource() {
    let repository = Arc::new(MemoryRepository::default());
    let path = PathBuf::from("shared.md");
    insert_file(&repository, path.clone(), "saved");
    let documents = Arc::new(MemoryDocuments::new(repository.clone()));
    let original = documents.open(&path).unwrap();
    documents
        .edit(
            &original.id,
            original.document.revision(),
            0..5,
            "remote draft",
        )
        .unwrap();
    let mut editor = EditorSession::new_empty(
        repository,
        Arc::new(FixedDialog {
            workspace: None,
            save: None,
        }),
        Arc::new(MemoryState::default()),
        documents.clone(),
    );
    editor
        .restore_snapshot(WorkspaceSnapshot {
            root: None,
            open_documents: vec![DocumentViewState {
                path: path.clone(),
                document_id: Some(DocumentId::new("previous-runtime-id")),
                draft_content: Some("old backup".to_owned()),
                ..DocumentViewState::default()
            }],
            active_path: Some(path),
            active_document_id: None,
        })
        .unwrap();
    assert_eq!(
        documents
            .attach(&original.id)
            .unwrap()
            .document
            .content_for_save(),
        "remote draft"
    );
    assert_eq!(editor.tabs().len(), 2);
    let recovered = editor
        .tabs()
        .iter()
        .find(|tab| tab.id != original.id)
        .unwrap()
        .id
        .clone();
    editor.activate_document(&recovered);
    assert_eq!(editor.active_text().unwrap(), "old backup");
    assert!(editor.active_path().is_none());
}

#[test]
fn restore_missing_file_recovers_draft_as_canonical_untitled_and_persists_it() {
    let repository = Arc::new(MemoryRepository::default());
    let documents = Arc::new(MemoryDocuments::new(repository.clone()));
    let state = Arc::new(MemoryState::default());
    let path = PathBuf::from("deleted.md");
    let draft = "unsaved 😀\nrecovered";
    let mut editor = EditorSession::new_empty(
        repository.clone(),
        Arc::new(FixedDialog {
            workspace: None,
            save: None,
        }),
        state.clone(),
        documents.clone(),
    );
    editor
        .restore_snapshot(WorkspaceSnapshot {
            root: None,
            open_documents: vec![DocumentViewState {
                document_id: Some(DocumentId::new("previous-runtime-id")),
                path: path.clone(),
                draft_content: Some(draft.to_owned()),
                ..DocumentViewState::default()
            }],
            active_path: Some(path.clone()),
            active_document_id: None,
        })
        .unwrap();
    assert_eq!(editor.tabs().len(), 1);
    assert_eq!(editor.active_text().unwrap(), draft);
    assert!(editor.active_path().is_none());
    assert!(editor.is_dirty());
    assert!(editor.restore_warning().is_some());
    let recovered = documents
        .attach(editor.active_document_id().unwrap())
        .unwrap();
    assert_eq!(recovered.document.content_for_save(), draft);
    assert!(recovered.document.path().is_none());
    editor.update_active_view(0..0, 0, 0.0, 0.0);
    let persisted = state.load().unwrap().unwrap();
    assert_eq!(persisted.open_documents.len(), 1);
    assert!(persisted.open_documents[0].path.as_os_str().is_empty());
    assert_eq!(
        persisted.open_documents[0].draft_content.as_deref(),
        Some(draft)
    );
    assert!(repository.fingerprint(&path).unwrap().is_none());
    editor.restore_snapshot(persisted).unwrap();
    assert_eq!(editor.active_text().unwrap(), draft);
}

#[test]
fn restore_revision_conflict_preserves_both_concurrent_content_and_backup_draft() {
    let repository = Arc::new(MemoryRepository::default());
    let path = PathBuf::from("shared.md");
    insert_file(&repository, path.clone(), "saved");
    let documents = Arc::new(MemoryDocuments::new(repository.clone()));
    *documents.concurrent_edit.lock().unwrap() = Some("newer remote draft".to_owned());
    let state = Arc::new(MemoryState::default());
    let mut editor = EditorSession::new_empty(
        repository.clone(),
        Arc::new(FixedDialog {
            workspace: None,
            save: None,
        }),
        state.clone(),
        documents.clone(),
    );
    editor
        .restore_snapshot(WorkspaceSnapshot {
            root: None,
            open_documents: vec![DocumentViewState {
                document_id: Some(DocumentId::new("previous-runtime-id")),
                path: path.clone(),
                draft_content: Some("old backup 😀".to_owned()),
                ..DocumentViewState::default()
            }],
            active_path: Some(path.clone()),
            active_document_id: None,
        })
        .unwrap();
    assert_eq!(editor.tabs().len(), 2);
    assert_eq!(editor.active_text().unwrap(), "newer remote draft");
    let shared = documents.open(&path).unwrap();
    assert_eq!(shared.document.content_for_save(), "newer remote draft");
    let recovered = editor
        .tabs()
        .into_iter()
        .find(|tab| tab.path.is_none())
        .unwrap();
    assert_ne!(recovered.id, shared.id);
    assert!(editor.activate_document(&recovered.id));
    assert_eq!(editor.active_text().unwrap(), "old backup 😀");
    assert!(editor.is_dirty());
    assert!(editor.restore_warning().is_some());
    assert_eq!(
        documents
            .attach(&recovered.id)
            .unwrap()
            .document
            .content_for_save(),
        "old backup 😀"
    );
    let persisted = state.load().unwrap().unwrap();
    assert!(
        persisted
            .open_documents
            .iter()
            .any(|view| view.path.as_os_str().is_empty()
                && view.draft_content.as_deref() == Some("old backup 😀"))
    );
    editor.restore_snapshot(persisted).unwrap();
    assert!(editor.activate_document(&recovered.id));
    assert_eq!(editor.active_text().unwrap(), "old backup 😀");
    assert_eq!(
        documents
            .attach(&shared.id)
            .unwrap()
            .document
            .content_for_save(),
        "newer remote draft"
    );
    assert_eq!(repository.read_file(&path).unwrap().bytes, b"saved");
}

#[test]
fn task_ids_are_unique_across_independent_window_sessions() {
    let root = WorkspaceId::new("canonical-workspace");
    let backend = Arc::new(MemoryTaskBackend::default());
    let mut first = TaskSession::new(backend.clone());
    let mut second = TaskSession::new(backend.clone());
    let mut ids = std::collections::HashSet::new();
    for _ in 0..32 {
        assert!(ids.insert(first.start_codex(root.clone(), "first").unwrap()));
        assert!(ids.insert(second.start_codex(root.clone(), "second").unwrap()));
    }
    let records = backend.records.lock().unwrap();
    assert_eq!(records.len(), 64);
    assert_eq!(
        records
            .iter()
            .map(|record| record.task.id.clone())
            .collect::<std::collections::HashSet<_>>()
            .len(),
        64
    );
}

#[test]
fn terminal_snapshot_atomically_replaces_cache_and_rejects_wrong_or_stale_resources() {
    let backend = Arc::new(ScriptedTerminalBackend {
        events: Mutex::new(Vec::new()),
    });
    let mut session = TerminalSession::new(backend.clone());
    let id = session.start(Path::new("repo"), 80, 24).unwrap();
    let mut snapshot = session.terminals()[0].clone();
    snapshot.revision = 10;
    snapshot.screen.rows[0].cells[0].text = "accepted".to_owned();
    backend
        .events
        .lock()
        .unwrap()
        .push(TerminalEvent::Snapshot(snapshot.clone()));
    assert!(session.refresh().unwrap());
    assert_eq!(session.terminals()[0].screen.rows[0].cells[0].text, "accepted");
    assert_eq!(session.terminals()[0].revision, 10);
    snapshot.revision = 9;
    snapshot.status = TerminalStatus::Failed;
    backend
        .events
        .lock()
        .unwrap()
        .push(TerminalEvent::Snapshot(snapshot.clone()));
    assert!(!session.refresh().unwrap());
    snapshot.revision = 11;
    snapshot.id = TerminalId::new("foreign");
    backend
        .events
        .lock()
        .unwrap()
        .push(TerminalEvent::Snapshot(snapshot.clone()));
    assert!(!session.refresh().unwrap());
    snapshot.id = id;
    snapshot.screen.rows[0].cells[0].text = "recovered".to_owned();
    snapshot.status = TerminalStatus::Exited;
    backend
        .events
        .lock()
        .unwrap()
        .push(TerminalEvent::Snapshot(snapshot));
    assert!(session.refresh().unwrap());
    assert_eq!(session.terminals()[0].screen.rows[0].cells[0].text, "recovered");
    assert_eq!(session.terminals()[0].status, TerminalStatus::Exited);
    assert_eq!(session.terminals()[0].revision, 11);
}

#[test]
fn failed_workspace_restore_retains_saved_root_without_authorizing_operations() {
    struct UnavailableTree {
        fail: std::sync::atomic::AtomicBool,
    }
    impl WorkspaceRepository for UnavailableTree {
        fn create_directory(&self, _: &Path, _: &str) -> Result<(), WorkspaceError> {
            panic!("Unavailable Workspace must not receive filesystem operations")
        }
        fn list_tree(&self, _: &Path) -> Result<Vec<FileEntry>, WorkspaceError> {
            if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
                Err(WorkspaceError::new("tree temporarily unavailable"))
            } else {
                Ok(Vec::new())
            }
        }
    }
    for registration_fails in [true, false] {
        let files = Arc::new(MemoryRepository::default());
        let connection = Arc::new(MemoryDocuments::new(files));
        connection.workspace_failure.store(registration_fails, std::sync::atomic::Ordering::SeqCst);
        let repository = Arc::new(UnavailableTree { fail: std::sync::atomic::AtomicBool::new(!registration_fails) });
        let state = Arc::new(MemoryState::default());
        let mut editor = EditorSession::new_empty(
            repository.clone(), Arc::new(FixedDialog { workspace: None, save: None }),
            state.clone(), connection.clone(),
        );
        let root = PathBuf::from("temporarily-offline");
        editor.restore_snapshot(WorkspaceSnapshot {
            root: Some(root.clone()),
            open_documents: vec![DocumentViewState {
                draft_content: Some("recovered input".into()),
                ..Default::default()
            }],
            ..Default::default()
        }).unwrap();
        assert!(editor.workspace_root().is_none());
        assert!(editor.workspace_id().is_none());
        assert!(editor.new_folder().is_err());
        assert!(editor.restore_warning().is_some());
        assert_eq!(editor.active_text().unwrap(), "recovered input");
        editor.persist().unwrap();
        let saved = state.load().unwrap().unwrap();
        assert_eq!(saved.root, Some(root.clone()));
        editor.restore_snapshot(saved.clone()).unwrap();
        assert_eq!(editor.snapshot().root, Some(root.clone()));
        connection.workspace_failure.store(false, std::sync::atomic::Ordering::SeqCst);
        repository.fail.store(false, std::sync::atomic::Ordering::SeqCst);
        editor.restore_snapshot(saved).unwrap();
        assert_eq!(editor.workspace_root(), Some(root.as_path()));
        let replacement = PathBuf::from("explicitly-opened");
        editor.open_workspace(replacement.clone()).unwrap();
        assert_eq!(state.load().unwrap().unwrap().root, Some(replacement));
        editor.close_workspace().unwrap();
        assert!(state.load().unwrap().unwrap().root.is_none());
        assert!(editor.snapshot().root.is_none());
    }
}

#[test]
fn explicit_workspace_close_clears_a_root_retained_after_restore_failure() {
    let repository = Arc::new(MemoryRepository::default());
    let connection = Arc::new(MemoryDocuments::new(repository.clone()));
    connection.workspace_failure.store(true, std::sync::atomic::Ordering::SeqCst);
    let state = Arc::new(MemoryState::default());
    let mut editor = EditorSession::new_empty(
        repository, Arc::new(FixedDialog { workspace: None, save: None }), state.clone(), connection,
    );
    editor.restore_snapshot(WorkspaceSnapshot {
        root: Some(PathBuf::from("offline")), ..Default::default()
    }).unwrap();
    editor.persist().unwrap();
    assert!(state.load().unwrap().unwrap().root.is_some());
    editor.close_workspace().unwrap();
    assert!(state.load().unwrap().unwrap().root.is_none());
}

#[test]
fn untitled_selection_survives_conversation_switching() {
    let repository = Arc::new(MemoryRepository::default());
    let path = PathBuf::from("original.txt");
    insert_file(&repository, path.clone(), "saved");
    let mut editor = session(repository, Arc::new(MemoryState::default()));
    editor.open_path(path).unwrap();
    editor.recover_draft("selected recovery").unwrap();
    let selected = editor.active_document_id().unwrap().clone();
    let mut conversations = ConversationSession::new(
        Arc::new(MemoryConversationState::default()), editor.snapshot(),
    );
    let first = conversations.active_id().clone();
    conversations.capture(&mut editor, ConversationViewState::default(), None, &[]).unwrap();
    conversations.create(&mut editor, ConversationViewState::default()).unwrap();
    editor.new_document();
    conversations.capture(&mut editor, ConversationViewState::default(), None, &[]).unwrap();
    conversations.switch(&first, &mut editor).unwrap();
    assert_eq!(editor.active_document_id(), Some(&selected));
    assert_eq!(editor.active_text().unwrap(), "selected recovery");
}

#[test]
fn selected_document_identity_takes_precedence_with_legacy_path_fallback() {
    let repository = Arc::new(MemoryRepository::default());
    let first_path = PathBuf::from("first.txt");
    let second_path = PathBuf::from("second.txt");
    insert_file(&repository, first_path.clone(), "first");
    insert_file(&repository, second_path.clone(), "second");
    let mut editor = session(repository, Arc::new(MemoryState::default()));
    editor.open_path(first_path.clone()).unwrap();
    let first = editor.active_document_id().unwrap().clone();
    editor.open_path(second_path.clone()).unwrap();
    let mut snapshot = editor.snapshot();
    snapshot.active_document_id = Some(first.clone());
    editor.restore_snapshot(snapshot.clone()).unwrap();
    assert_eq!(editor.active_document_id(), Some(&first));
    assert_eq!(editor.active_path(), Some(first_path.as_path()));
    snapshot.active_document_id = None;
    editor.restore_snapshot(snapshot).unwrap();
    assert_eq!(editor.active_path(), Some(second_path.as_path()));
}
