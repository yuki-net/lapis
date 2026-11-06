use lapis_client_api::{
    DocumentEditRequest, DocumentHistoryRequest, DocumentInspectRequest, DocumentSaveRequest,
    DocumentTextEdit, DocumentTransaction, RequestBody, ResponseBody, Revision as WireRevision,
};
use lapis_document::{DocumentErrorKind, ExternalChange};
use lapis_host::BackendHost;

#[test]
fn dirty_document_can_reconnect_after_the_entire_workspace_directory_is_deleted() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("draft.md");
    std::fs::write(&path, "base").unwrap();
    let host = BackendHost::start().unwrap();
    let first = host.documents();
    let opened = first.open(&path).unwrap();
    let dirty = first
        .edit(&opened.id, opened.document.revision(), 0..4, "unsaved")
        .unwrap();
    drop(first);
    root.close().unwrap();
    let second = host.documents();
    let restored = second.attach(&opened.id).unwrap();
    assert_eq!(restored.document, dirty.document);
    assert_eq!(restored.external_change, ExternalChange::Deleted);
    let edited = second
        .edit(
            &opened.id,
            restored.document.revision(),
            0..7,
            "reconnected",
        )
        .unwrap();
    assert_eq!(edited.document.content_for_save(), "reconnected");
    assert!(
        second
            .save(&opened.id, edited.document.revision(), None)
            .is_err()
    );
    assert_eq!(
        second
            .attach(&opened.id)
            .unwrap()
            .document
            .content_for_save(),
        "reconnected"
    );
}

#[test]
fn deleting_the_parent_directory_does_not_hide_the_canonical_dirty_document() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("deleted");
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("draft.md");
    std::fs::write(&path, "base").unwrap();
    let host = BackendHost::start().unwrap();
    host.register_workspace(root.path()).unwrap();
    let documents = host.documents();
    let opened = documents.open(&path).unwrap();
    let dirty = documents
        .edit(&opened.id, opened.document.revision(), 0..4, "unsaved")
        .unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::remove_dir(&directory).unwrap();
    let current = documents.attach(&opened.id).unwrap();
    assert_eq!(current.document.content_for_save(), "unsaved");
    assert_eq!(current.external_change, ExternalChange::Deleted);
    let edited = documents
        .edit(
            &opened.id,
            dirty.document.revision(),
            0..7,
            "still editable",
        )
        .unwrap();
    assert_eq!(edited.document.content_for_save(), "still editable");
    assert_eq!(edited.external_change, ExternalChange::Deleted);
    assert!(edited.document.is_dirty());
}

#[test]
fn overlapping_workspace_grants_cannot_create_a_second_document_owner() {
    let root = tempfile::tempdir().unwrap();
    let child = root.path().join("child");
    std::fs::create_dir(&child).unwrap();
    let path = child.join("shared.md");
    std::fs::write(&path, "base").unwrap();
    let host = BackendHost::start().unwrap();
    let parent = host.connect(root.path()).unwrap();
    let documents = host.documents();
    let original = documents.open(&path).unwrap();
    let dirty = documents
        .edit(&original.id, original.document.revision(), 0..4, "unsaved")
        .unwrap();
    let nested = host.connect(&child).unwrap();
    let response = nested
        .request(RequestBody::DocumentOpen(
            lapis_client_api::DocumentOpenRequest {
                workspace_id: nested.workspace_id().clone(),
                path: lapis_client_api::WorkspaceRelativePath::parse("shared.md").unwrap(),
            },
        ))
        .unwrap();
    let ResponseBody::Error(error) = response else {
        panic!("overlapping scope created a duplicate Document")
    };
    assert_eq!(error.code.as_str(), "document_conflict");
    assert!(documents.open(&path).is_err());
    assert_eq!(
        documents.attach(&original.id).unwrap().document,
        dirty.document
    );
    drop(parent);
}

#[test]
fn attaching_a_document_skips_deleted_unrelated_workspace_roots() {
    let directory = tempfile::tempdir().unwrap();
    let stale = directory.path().join("a-stale");
    let valid = directory.path().join("z-valid");
    std::fs::create_dir(&stale).unwrap();
    std::fs::create_dir(&valid).unwrap();
    let path = valid.join("shared.md");
    std::fs::write(&path, "base").unwrap();
    let host = BackendHost::start().unwrap();
    host.register_workspace(&stale).unwrap();
    host.register_workspace(&valid).unwrap();
    let opened = host.documents().open(&path).unwrap();
    std::fs::remove_dir(&stale).unwrap();
    let attached = host.documents().attach(&opened.id).unwrap();
    assert_eq!(attached.id, opened.id);
    assert_eq!(attached.document.content_for_save(), "base");
}

#[test]
fn desktop_port_and_wire_share_identity_edits_history_and_saved_state() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("shared.md");
    std::fs::write(&path, "a😀b").unwrap();
    let host = BackendHost::start().unwrap();
    let client = host.connect(root.path()).unwrap();
    let desktop = host.documents();
    let first = desktop.open(&path).unwrap();
    let id = lapis_client_api::DocumentId::try_new(first.id.as_str()).unwrap();
    let inspect = || match client
        .request(RequestBody::DocumentInspect(DocumentInspectRequest {
            document_id: id.clone(),
        }))
        .unwrap()
    {
        ResponseBody::DocumentInspect(response) => response.document,
        response => panic!("unexpected response: {response:?}"),
    };
    let original = inspect();
    assert_eq!(original.content, "a😀b");
    let edited = desktop
        .edit(&first.id, first.document.revision(), 1..2, "界")
        .unwrap();
    assert_eq!(inspect().content, "a界b");
    let response = client
        .request(RequestBody::DocumentEdit(DocumentEditRequest {
            document_id: id.clone(),
            base_revision: WireRevision::new(edited.document.revision().number()),
            transaction: DocumentTransaction::try_new(vec![
                DocumentTextEdit::try_new(0, 1, "shared").unwrap(),
            ])
            .unwrap(),
        }))
        .unwrap();
    assert!(matches!(response, ResponseBody::DocumentEdit(_)));
    assert_eq!(
        desktop
            .edit(&first.id, edited.document.revision(), 0..1, "stale")
            .unwrap_err()
            .kind(),
        DocumentErrorKind::Conflict
    );
    let live = desktop.attach(&first.id).unwrap();
    assert_eq!(live.document.content_for_save(), "shared界b");
    let undone = desktop
        .history(&first.id, live.document.revision(), true)
        .unwrap();
    assert_eq!(undone.document.content_for_save(), "a界b");
    assert!(undone.can_redo);
    let response = client
        .request(RequestBody::DocumentRedo(DocumentHistoryRequest {
            document_id: id.clone(),
            base_revision: WireRevision::new(undone.document.revision().number()),
        }))
        .unwrap();
    assert!(matches!(response, ResponseBody::DocumentRedo(_)));
    let current = inspect();
    assert!(matches!(
        client
            .request(RequestBody::DocumentSave(DocumentSaveRequest {
                document_id: id.clone(),
                base_revision: current.revision
            }))
            .unwrap(),
        ResponseBody::DocumentSave(_)
    ));
    let saved = desktop.attach(&first.id).unwrap();
    assert!(!saved.document.is_dirty());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "shared界b");
    drop(desktop);
    drop(client);
    let reconnected = host.documents().attach(&first.id).unwrap();
    assert_eq!(reconnected.id, first.id);
    assert_eq!(reconnected.document.revision(), saved.document.revision());
    assert!(reconnected.can_undo);
}

#[test]
fn untitled_save_as_external_changes_and_reload_keep_one_monotonic_resource() {
    let root = tempfile::tempdir().unwrap();
    let host = BackendHost::start().unwrap();
    let desktop = host.documents();
    let original = desktop.create(Some(root.path())).unwrap();
    assert!(original.document.path().is_none());
    let edited = desktop
        .edit(&original.id, original.document.revision(), 0..0, "draft😀")
        .unwrap();
    let target = root.path().join("new.md");
    let saved = desktop
        .save(
            &original.id,
            edited.document.revision(),
            Some(target.clone()),
        )
        .unwrap();
    assert_eq!(saved.id, original.id);
    assert!(!saved.document.is_dirty());
    assert_eq!(desktop.open(&target).unwrap().id, original.id);
    std::fs::write(&target, "external").unwrap();
    let changed = desktop.attach(&original.id).unwrap();
    assert_eq!(changed.external_change, ExternalChange::Modified);
    let reloaded = desktop
        .reload(&original.id, changed.document.revision())
        .unwrap();
    assert!(reloaded.document.revision() > changed.document.revision());
    assert_eq!(reloaded.document.content_for_save(), "external");
    assert!(!reloaded.can_undo);
    assert_eq!(reloaded.external_change, ExternalChange::Unchanged);
    assert_eq!(
        desktop
            .edit(&original.id, saved.document.revision(), 0..0, "stale")
            .unwrap_err()
            .kind(),
        DocumentErrorKind::Conflict
    );
    let dirty = desktop
        .edit(&original.id, reloaded.document.revision(), 0..0, "local")
        .unwrap();
    assert_eq!(
        desktop
            .reload(&original.id, dirty.document.revision())
            .unwrap_err()
            .kind(),
        DocumentErrorKind::Conflict
    );
    std::fs::write(&target, "external again").unwrap();
    assert_eq!(
        desktop
            .save(&original.id, dirty.document.revision(), None)
            .unwrap_err()
            .kind(),
        DocumentErrorKind::Conflict
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "external again");
    std::fs::remove_file(&target).unwrap();
    assert_eq!(
        desktop.attach(&original.id).unwrap().external_change,
        ExternalChange::Deleted
    );
}

#[test]
fn save_as_refuses_existing_targets_without_losing_draft() {
    let root = tempfile::tempdir().unwrap();
    let host = BackendHost::start().unwrap();
    let desktop = host.documents();
    let original = desktop.create(Some(root.path())).unwrap();
    let edited = desktop
        .edit(&original.id, original.document.revision(), 0..0, "draft")
        .unwrap();
    let target = root.path().join("existing.md");
    std::fs::write(&target, "existing").unwrap();
    assert_eq!(
        desktop
            .save(
                &original.id,
                edited.document.revision(),
                Some(target.clone())
            )
            .unwrap_err()
            .kind(),
        DocumentErrorKind::Conflict
    );
    assert_eq!(std::fs::read_to_string(target).unwrap(), "existing");
    let recovered = desktop.attach(&original.id).unwrap();
    assert_eq!(recovered.document.content_for_save(), "draft");
    assert!(recovered.document.path().is_none());
}

#[test]
fn native_save_as_moves_the_resource_across_workspaces_without_resetting_history() {
    let source = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let host = BackendHost::start().unwrap();
    let first = host.documents();
    let second = host.documents();
    let original = first.create(Some(source.path())).unwrap();
    let old_view = second.attach(&original.id).unwrap();
    let edited = first
        .edit(&original.id, original.document.revision(), 0..0, "moved😀")
        .unwrap();
    let path = destination.path().join("moved.md");
    let saved = first
        .save(&original.id, edited.document.revision(), Some(path.clone()))
        .unwrap();
    assert_eq!(saved.id, original.id);
    assert_eq!(saved.document.revision(), edited.document.revision());
    assert!(saved.can_undo);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "moved😀");
    let recovered = second.attach(&old_view.id).unwrap();
    assert_eq!(recovered.id, saved.id);
    assert_eq!(recovered.document.path(), saved.document.path());
    assert_eq!(first.open(&path).unwrap().id, saved.id);
    let undone = second
        .history(&original.id, saved.document.revision(), true)
        .unwrap();
    assert_eq!(undone.document.content_for_save(), "");
    assert!(undone.document.is_dirty());
    assert!(undone.can_redo);
}

#[test]
fn closing_one_window_detaches_only_its_lease_and_preserves_shared_dirty_edits() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("shared.md");
    std::fs::write(&path, "base").unwrap();
    let host = BackendHost::start().unwrap();
    let first = host.documents();
    let second = first.fork();
    let opened = first.open(&path).unwrap();
    let another = second.open(&path).unwrap();
    assert_eq!(opened.id, another.id);
    let dirty = first
        .edit(&opened.id, opened.document.revision(), 0..4, "dirty")
        .unwrap();
    first.close(&opened.id).unwrap();
    let attached = second.attach(&another.id).unwrap();
    assert_eq!(attached.document.content_for_save(), "dirty");
    let edited = second
        .edit(&another.id, dirty.document.revision(), 0..5, "other window")
        .unwrap();
    second.close(&another.id).unwrap();
    drop(first);
    drop(second);
    let reopened = host.documents().open(&path).unwrap();
    assert_eq!(reopened.id, opened.id);
    assert_eq!(reopened.document.revision(), edited.document.revision());
    assert_eq!(reopened.document.content_for_save(), "other window");
    assert!(reopened.document.is_dirty());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "base");
}

#[test]
fn same_workspace_path_aliases_reuse_one_canonical_document() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("Shared.md");
    std::fs::write(&path, "base").unwrap();
    let host = BackendHost::start().unwrap();
    let client = host.connect(root.path()).unwrap();
    let open = |path: &str| match client
        .request(RequestBody::DocumentOpen(
            lapis_client_api::DocumentOpenRequest {
                workspace_id: client.workspace_id().clone(),
                path: lapis_client_api::WorkspaceRelativePath::parse(path).unwrap(),
            },
        ))
        .unwrap()
    {
        ResponseBody::DocumentOpen(response) => response.document,
        response => panic!("unexpected response: {response:?}"),
    };
    let original = open("Shared.md");
    #[cfg(windows)]
    assert_eq!(open("shared.md").document_id, original.document_id);
    let alias = root.path().join("alias.md");
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_file(&path, &alias).is_ok();
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&path, &alias).is_ok();
    #[cfg(not(any(windows, unix)))]
    let linked = false;
    if linked {
        let aliased = open("alias.md");
        assert_eq!(aliased.document_id, original.document_id);
        assert_eq!(aliased.path, original.path);
    }
}
