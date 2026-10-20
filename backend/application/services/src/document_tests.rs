struct MemoryDocuments {
    repository: Arc<MemoryRepository>,
    documents: Arc<Mutex<HashMap<DocumentId, Document>>>,
    concurrent_edit: Arc<Mutex<Option<String>>>,
}

impl MemoryDocuments {
    fn new(repository: Arc<MemoryRepository>) -> Self {
        Self {
            repository,
            documents: Arc::new(Mutex::new(HashMap::new())),
            concurrent_edit: Arc::new(Mutex::new(None)),
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
        })
    }
    fn workspace(&self, _: &Path) -> Result<WorkspaceId, DocumentError> {
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
        self.mutate(id, revision, |document| {
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
        })
    }

    fn close(&self, _: &DocumentId) -> Result<(), DocumentError> {
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
    snapshot.output = vec![42; 256 * 1024 + 10];
    snapshot.output_sequence = 10;
    backend
        .events
        .lock()
        .unwrap()
        .push(TerminalEvent::Snapshot(snapshot.clone()));
    assert!(session.refresh().unwrap());
    assert_eq!(session.terminals()[0].output.len(), 256 * 1024);
    assert!(session.terminals()[0].output_truncated);
    assert_eq!(session.terminals()[0].output_sequence, 10);
    snapshot.output_sequence = 9;
    snapshot.status = TerminalStatus::Failed;
    backend
        .events
        .lock()
        .unwrap()
        .push(TerminalEvent::Snapshot(snapshot.clone()));
    assert!(!session.refresh().unwrap());
    snapshot.output_sequence = 11;
    snapshot.id = TerminalId::new("foreign");
    backend
        .events
        .lock()
        .unwrap()
        .push(TerminalEvent::Snapshot(snapshot.clone()));
    assert!(!session.refresh().unwrap());
    snapshot.id = id;
    snapshot.output = b"recovered".to_vec();
    snapshot.status = TerminalStatus::Exited;
    backend
        .events
        .lock()
        .unwrap()
        .push(TerminalEvent::Snapshot(snapshot));
    assert!(session.refresh().unwrap());
    assert_eq!(session.terminals()[0].output, b"recovered");
    assert_eq!(session.terminals()[0].status, TerminalStatus::Exited);
    assert_eq!(session.terminals()[0].output_sequence, 11);
}
