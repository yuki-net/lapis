use lapis_client_api::{DocumentDiscardRequest, EventBody, RequestBody, ResponseBody};
use lapis_document::{DocumentErrorKind, ExternalChange};
use lapis_host::BackendHost;
use std::time::Duration;

#[test]
fn discard_close_reopen_restores_saved_content_and_publishes_replacement() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("note.md");
    std::fs::write(&path, "saved😀").unwrap();
    let host = BackendHost::start().unwrap();
    let documents = host.documents();
    let original = documents.open(&path).unwrap();
    let id = original.id;
    let dirty = documents
        .edit(&id, original.document.revision(), 0..6, "discard me")
        .unwrap();
    let revision = dirty.document.revision();
    let observer = host.connect(root.path()).unwrap();
    let events = observer.subscribe().unwrap();
    documents.discard(&id, revision).unwrap();
    documents.close(&id).unwrap();
    let event = events.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(
        matches!(event.body, EventBody::DocumentReplaced { document }
        if document.document_id.as_str() == id.as_str() && document.content == "saved😀" && !document.dirty)
    );
    let reopened = host.documents().open(&path).unwrap();
    assert_eq!(reopened.id, id);
    assert_eq!(reopened.document.content_for_save(), "saved😀");
    assert!(reopened.document.revision() > revision);
    assert!(!reopened.document.is_dirty());
    assert!(!reopened.can_undo);
    assert!(!reopened.can_redo);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "saved😀");
}

#[test]
fn stale_or_unattached_discard_never_drops_another_clients_changes() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("note.md");
    std::fs::write(&path, "saved").unwrap();
    let host = BackendHost::start().unwrap();
    let first = host.documents();
    let second = first.fork();
    let original = first.open(&path).unwrap();
    let other = second.open(&path).unwrap();
    let changed = second
        .edit(&other.id, other.document.revision(), 0..5, "newer draft")
        .unwrap();
    let outsider = host.connect(root.path()).unwrap();
    let request = DocumentDiscardRequest {
        document_id: lapis_client_api::DocumentId::try_new(original.id.as_str()).unwrap(),
        base_revision: lapis_client_api::Revision::new(changed.document.revision().number()),
    };
    assert!(matches!(
        outsider
            .request(RequestBody::DocumentDiscard(request))
            .unwrap(),
        ResponseBody::Error(_)
    ));
    assert_eq!(
        first
            .discard(&original.id, original.document.revision())
            .unwrap_err()
            .kind(),
        DocumentErrorKind::Conflict
    );
    let retained = second.attach(&other.id).unwrap();
    assert_eq!(retained.document.content_for_save(), "newer draft");
    assert_eq!(retained.document.revision(), changed.document.revision());
    assert!(retained.document.is_dirty());
    assert!(retained.can_undo);
    assert_eq!(std::fs::read_to_string(path).unwrap(), "saved");
}

#[test]
fn deleted_saved_file_discard_conflicts_while_external_saved_content_is_reloaded_without_writes() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("note.md");
    std::fs::write(&path, "saved").unwrap();
    let host = BackendHost::start().unwrap();
    let documents = host.documents();
    let original = documents.open(&path).unwrap();
    let dirty = documents
        .edit(
            &original.id,
            original.document.revision(),
            0..5,
            "keep draft",
        )
        .unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        documents
            .discard(&original.id, dirty.document.revision())
            .unwrap_err()
            .kind(),
        DocumentErrorKind::Conflict
    );
    let retained = documents.attach(&original.id).unwrap();
    assert_eq!(retained.document.content_for_save(), "keep draft");
    assert_eq!(retained.document.revision(), dirty.document.revision());
    assert!(retained.document.is_dirty());
    assert_eq!(retained.external_change, ExternalChange::Deleted);
    assert!(!path.exists());
    std::fs::write(&path, "external saved").unwrap();
    let discarded = documents
        .discard(&original.id, dirty.document.revision())
        .unwrap();
    assert_eq!(discarded.document.content_for_save(), "external saved");
    assert!(!discarded.document.is_dirty());
    assert!(!discarded.can_undo);
    assert_eq!(std::fs::read_to_string(path).unwrap(), "external saved");
}

#[test]
fn untitled_and_never_saved_named_documents_discard_to_empty_without_reserving_a_path() {
    let root = tempfile::tempdir().unwrap();
    let host = BackendHost::start().unwrap();
    let documents = host.documents();
    let original = documents.create(Some(root.path())).unwrap();
    let dirty = documents
        .edit(&original.id, original.document.revision(), 0..0, "draft")
        .unwrap();
    let discarded = documents
        .discard(&original.id, dirty.document.revision())
        .unwrap();
    assert_eq!(discarded.id, original.id);
    assert!(discarded.document.is_empty());
    assert!(discarded.document.path().is_none());
    assert!(!discarded.document.is_dirty());
    assert!(!discarded.can_undo);
    assert!(discarded.document.revision() > dirty.document.revision());
    let client = host.connect(root.path()).unwrap();
    let create = || {
        RequestBody::DocumentCreate(lapis_client_api::DocumentCreateRequest {
            workspace_id: client.workspace_id().clone(),
            path: lapis_client_api::WorkspaceRelativePath::parse("new.md").unwrap(),
            encoding: lapis_client_api::DocumentEncoding::Utf8,
            content: "never saved".to_owned(),
        })
    };
    let ResponseBody::DocumentCreate(created) = client.request(create()).unwrap() else {
        panic!("create failed")
    };
    let named = documents
        .attach(&lapis_editor_core::DocumentId::new(
            created.document.document_id.as_str(),
        ))
        .unwrap();
    let discarded = documents
        .discard(&named.id, named.document.revision())
        .unwrap();
    assert!(discarded.document.path().is_none());
    assert!(discarded.document.is_empty());
    assert!(!discarded.document.is_dirty());
    assert!(matches!(
        client.request(create()).unwrap(),
        ResponseBody::DocumentCreate(_)
    ));
    assert!(!root.path().join("new.md").exists());
}
