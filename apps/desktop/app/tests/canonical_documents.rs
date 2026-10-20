use lapis_host::BackendHost;

#[test]
fn deleted_workspace_keeps_recovered_drafts_in_an_untitled_document() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("deleted.md");
    let snapshot = lapis_desktop_client::WorkspaceSnapshot {
        root: Some(root.path().to_owned()),
        open_documents: vec![lapis_desktop_client::DocumentViewState {
            path,
            draft_content: Some("unsaved workspace draft 😀".into()),
            ..Default::default()
        }],
        ..Default::default()
    };
    root.close().unwrap();
    let host = BackendHost::start().unwrap();
    let store = std::sync::Arc::new(ViewStore::default());
    let mut editor = lapis_desktop_client::EditorSession::new_empty(
        host.workspaces(),
        std::sync::Arc::new(NoDialog),
        store.clone(),
        host.documents(),
    );
    editor.restore_snapshot(snapshot).unwrap();
    assert_eq!(editor.active_text().unwrap(), "unsaved workspace draft 😀");
    assert!(editor.workspace_root().is_none());
    assert!(editor.active_path().is_none());
    assert!(editor.is_dirty());
    assert!(editor.restore_warning().is_some());
    editor.update_active_view(0..0, 0, 0.0, 0.0);
    let saved = lapis_desktop_client::WorkspaceStateRepository::load(&*store)
        .unwrap()
        .unwrap();
    assert!(saved.root.is_none());
    assert_eq!(
        saved.open_documents[0].draft_content.as_deref(),
        Some("unsaved workspace draft 😀")
    );
    editor.restore_snapshot(saved).unwrap();
    assert_eq!(editor.active_text().unwrap(), "unsaved workspace draft 😀");
}

#[derive(Default)]
struct ViewStore(std::sync::Mutex<Option<lapis_desktop_client::WorkspaceSnapshot>>);

impl lapis_desktop_client::WorkspaceStateRepository for ViewStore {
    fn load(
        &self,
    ) -> Result<Option<lapis_desktop_client::WorkspaceSnapshot>, lapis_workspace::WorkspaceError>
    {
        Ok(self.0.lock().unwrap().clone())
    }
    fn save(
        &self,
        snapshot: &lapis_desktop_client::WorkspaceSnapshot,
    ) -> Result<(), lapis_workspace::WorkspaceError> {
        *self.0.lock().unwrap() = Some(snapshot.clone());
        Ok(())
    }
}

struct NoDialog;

impl lapis_desktop_client::WorkspaceDialog for NoDialog {
    fn choose_workspace_path(&self) -> Option<std::path::PathBuf> {
        None
    }
    fn choose_save_path(&self, _: &str) -> Option<std::path::PathBuf> {
        None
    }
}

#[test]
fn desktop_sessions_share_runtime_resources_across_close_restore_and_workspace_switches() {
    let root = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let path = root.path().join("shared.md");
    std::fs::write(&path, "base").unwrap();
    let host = BackendHost::start().unwrap();
    let documents = host.documents();
    let make_editor = || {
        lapis_desktop_client::EditorSession::new_empty(
            host.workspaces(),
            std::sync::Arc::new(NoDialog),
            std::sync::Arc::new(ViewStore::default()),
            documents.clone(),
        )
    };
    let mut first = make_editor();
    let mut second = make_editor();
    first.open_workspace(root.path().to_owned()).unwrap();
    second.open_workspace(root.path().to_owned()).unwrap();
    first.open_path(path.clone()).unwrap();
    second.open_path(path.clone()).unwrap();
    let id = first.active_document_id().unwrap().clone();
    assert_eq!(second.active_document_id(), Some(&id));
    first.replace_range(0..4, "first draft").unwrap();
    let old_view = first.snapshot();
    second.poll_external_changes();
    second.replace_range(0..11, "newer remote draft").unwrap();
    first.restore_snapshot(old_view.clone()).unwrap();
    assert_eq!(first.active_text().unwrap(), "newer remote draft");
    first.document_connection().close(&id).unwrap();
    drop(first);
    second.replace_range(0..18, "still attached").unwrap();
    second.open_workspace(other.path().to_owned()).unwrap();
    second.restore_snapshot(old_view).unwrap();
    assert_eq!(second.active_document_id(), Some(&id));
    assert_eq!(second.active_text().unwrap(), "still attached");
    assert!(second.undo());
    assert_eq!(second.active_text().unwrap(), "newer remote draft");
    assert!(second.redo());
    second.save_document().unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "still attached");
    drop(second);
    let reopened = host.documents().open(&path).unwrap();
    assert_eq!(reopened.id, id);
    assert!(reopened.can_undo);
    assert!(!reopened.document.is_dirty());
}
