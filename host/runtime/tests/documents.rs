use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::Path,
    sync::Arc,
    time::Duration,
};

use backend_state::{
    BackendService, BackendSession, BackendState, WorkspaceEntry, WorkspaceEntryKind,
    WorkspaceFileBackend, WorkspaceRegistration,
};
use client_api::{
    DocumentClaimRequest, DocumentCloseRequest, DocumentCreateRequest, DocumentEditRequest,
    DocumentEncoding, DocumentExternalChange, DocumentHistoryRequest, DocumentOpenRequest,
    DocumentSaveRequest, DocumentTextEdit, DocumentTransaction, EventBody, FileTreeRequest,
    REVISION_CONFLICT, RequestBody, RequestEnvelope, RequestId, ResponseBody, Revision, SessionId,
    SnapshotReason, SnapshotRequest, WorkspaceCloseRequest, WorkspaceConnectRequest, WorkspaceId,
    WorkspaceRelativePath,
};
use document::{DocumentError, DocumentRepository, FileData, FileFingerprint};

#[derive(Default)]
struct TestFiles;

impl DocumentRepository for TestFiles {
    fn read_file(&self, path: &Path) -> Result<FileData, DocumentError> {
        let bytes = fs::read(path).map_err(|error| DocumentError::io(error.to_string()))?;
        Ok(FileData {
            fingerprint: fingerprint(path, &bytes)?,
            bytes,
        })
    }

    fn write_file(
        &self,
        path: &Path,
        content: &[u8],
        expected: Option<&FileFingerprint>,
    ) -> Result<FileFingerprint, DocumentError> {
        let actual = self.fingerprint(path)?;
        if expected != actual.as_ref() {
            return Err(DocumentError::conflict("external file change"));
        }
        fs::write(path, content).map_err(|error| DocumentError::io(error.to_string()))?;
        fingerprint(path, content)
    }

    fn fingerprint(&self, path: &Path) -> Result<Option<FileFingerprint>, DocumentError> {
        match fs::read(path) {
            Ok(bytes) => fingerprint(path, &bytes).map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(DocumentError::io(error.to_string())),
        }
    }
}

impl WorkspaceFileBackend for TestFiles {
    fn create_directory(&self, _: &std::path::Path) -> Result<(), document::DocumentError> {
        Err(document::DocumentError::io("Unsupported"))
    }
    fn list_children(&self, directory: &Path) -> Result<Vec<WorkspaceEntry>, DocumentError> {
        let mut entries = fs::read_dir(directory)
            .map_err(|error| DocumentError::io(error.to_string()))?
            .map(|entry| {
                let entry = entry.map_err(|error| DocumentError::io(error.to_string()))?;
                let metadata = fs::symlink_metadata(entry.path())
                    .map_err(|error| DocumentError::io(error.to_string()))?;
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| DocumentError::io("non-UTF-8 file name"))?;
                let kind = if metadata.file_type().is_symlink() {
                    WorkspaceEntryKind::Symlink
                } else if metadata.is_dir() {
                    WorkspaceEntryKind::Directory
                } else {
                    WorkspaceEntryKind::File
                };
                Ok(WorkspaceEntry { name, kind })
            })
            .collect::<Result<Vec<_>, DocumentError>>()?;
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(entries)
    }
}

#[test]
fn workspace_files_document_revision_events_and_snapshot_form_one_flow() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("src")).unwrap();
    fs::write(root.path().join("src/main.ts"), "a😀b").unwrap();
    let (service, session, workspace_id) = service(root.path(), "workspace-1", "session-1");
    assert!(service.subscribe(session.clone()).is_err());
    let before_connect = dispatch(
        &service,
        &session,
        1,
        RequestBody::FileTree(FileTreeRequest {
            workspace_id: workspace_id.clone(),
            path: None,
        }),
    );
    assert!(matches!(before_connect, ResponseBody::Error(_)));

    assert!(matches!(
        dispatch(
            &service,
            &session,
            2,
            RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
                workspace_id: workspace_id.clone(),
            }),
        ),
        ResponseBody::WorkspaceConnect(_)
    ));
    let events = service.subscribe(session.clone()).unwrap();

    let ResponseBody::FileTree(tree) = dispatch(
        &service,
        &session,
        3,
        RequestBody::FileTree(FileTreeRequest {
            workspace_id: workspace_id.clone(),
            path: None,
        }),
    ) else {
        panic!("expected file tree");
    };
    assert_eq!(tree.entries.len(), 1);
    assert_eq!(tree.entries[0].name, "src");

    let ResponseBody::DocumentOpen(opened) = dispatch(
        &service,
        &session,
        4,
        RequestBody::DocumentOpen(DocumentOpenRequest {
            workspace_id: workspace_id.clone(),
            path: WorkspaceRelativePath::parse("src/main.ts").unwrap(),
        }),
    ) else {
        panic!("expected opened document");
    };
    assert_eq!(opened.document.revision, Revision::new(1));
    assert!(matches!(
        events.recv_timeout(Duration::from_secs(1)).unwrap().body,
        EventBody::DocumentReplaced { .. }
    ));

    let transaction =
        DocumentTransaction::try_new(vec![DocumentTextEdit::try_new(1, 2, "日本").unwrap()])
            .unwrap();
    let ResponseBody::DocumentEdit(edited) = dispatch(
        &service,
        &session,
        5,
        RequestBody::DocumentEdit(DocumentEditRequest {
            document_id: opened.document.document_id.clone(),
            base_revision: Revision::new(1),
            transaction: transaction.clone(),
        }),
    ) else {
        panic!("expected edit response");
    };
    assert_eq!(edited.revision, Revision::new(2));
    assert!(matches!(
        events.recv_timeout(Duration::from_secs(1)).unwrap().body,
        EventBody::DocumentEdited { revision, .. } if revision == Revision::new(2)
    ));

    let stale = dispatch(
        &service,
        &session,
        6,
        RequestBody::DocumentEdit(DocumentEditRequest {
            document_id: opened.document.document_id.clone(),
            base_revision: Revision::new(1),
            transaction,
        }),
    );
    assert!(matches!(
        stale,
        ResponseBody::Error(ref error)
            if error.code.as_str() == REVISION_CONFLICT
                && error.revision_conflict.as_ref().is_some_and(|conflict| {
                    conflict.expected == Revision::new(1) && conflict.actual == Revision::new(2)
                })
    ));

    assert!(matches!(
        dispatch(
            &service,
            &session,
            7,
            RequestBody::DocumentSave(DocumentSaveRequest {
                document_id: opened.document.document_id,
                base_revision: Revision::new(2),
            }),
        ),
        ResponseBody::DocumentSave(_)
    ));
    assert_eq!(
        fs::read_to_string(root.path().join("src/main.ts")).unwrap(),
        "a日本b"
    );
    assert!(matches!(
        events.recv_timeout(Duration::from_secs(1)).unwrap().body,
        EventBody::DocumentSaved { revision, .. } if revision == Revision::new(2)
    ));

    let ResponseBody::SnapshotResync(snapshot) = dispatch(
        &service,
        &session,
        8,
        RequestBody::SnapshotResync(SnapshotRequest {
            workspace_id,
            reason: SnapshotReason::Reconnect,
        }),
    ) else {
        panic!("expected snapshot");
    };
    assert_eq!(snapshot.snapshot.documents[0].content, "a日本b");
    assert!(!snapshot.snapshot.documents[0].dirty);
    assert_eq!(snapshot.snapshot.event_watermark.value(), 3);

    assert!(matches!(
        dispatch(
            &service,
            &session,
            9,
            RequestBody::WorkspaceClose(WorkspaceCloseRequest {
                workspace_id: snapshot.snapshot.workspace.workspace_id,
            }),
        ),
        ResponseBody::WorkspaceClose(_)
    ));
    assert!(matches!(
        events.recv_timeout(Duration::from_secs(1)),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
    ));
}

#[test]
fn workspace_and_document_ownership_are_rechecked_in_backend_state() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    fs::write(first.path().join("a.ts"), "a").unwrap();
    fs::write(second.path().join("b.ts"), "b").unwrap();
    let first_id = WorkspaceId::try_new("workspace-1").unwrap();
    let second_id = WorkspaceId::try_new("workspace-2").unwrap();
    let state = BackendState::new([
        WorkspaceRegistration::new(
            first_id.clone(),
            "First",
            first.path().to_owned(),
            Arc::new(TestFiles),
        ),
        WorkspaceRegistration::new(
            second_id.clone(),
            "Second",
            second.path().to_owned(),
            Arc::new(TestFiles),
        ),
    ])
    .unwrap();
    let service = BackendService::start(state).unwrap();
    let first_session =
        BackendSession::new(SessionId::try_new("session-1").unwrap(), first_id.clone());
    let second_session =
        BackendSession::new(SessionId::try_new("session-2").unwrap(), second_id.clone());
    connect(&service, &first_session, first_id.clone(), 1);
    connect(&service, &second_session, second_id.clone(), 2);

    let reused_session = BackendSession::new(first_session.session_id.clone(), second_id.clone());
    let session_workspace_switch = dispatch(
        &service,
        &reused_session,
        3,
        RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
            workspace_id: second_id.clone(),
        }),
    );
    assert!(matches!(
        session_workspace_switch,
        ResponseBody::Error(ref error) if error.code.as_str() == "forbidden"
    ));

    let cross_workspace = dispatch(
        &service,
        &first_session,
        4,
        RequestBody::DocumentOpen(DocumentOpenRequest {
            workspace_id: second_id,
            path: WorkspaceRelativePath::parse("b.ts").unwrap(),
        }),
    );
    assert!(matches!(
        cross_workspace,
        ResponseBody::Error(ref error) if error.code.as_str() == "forbidden"
    ));

    let ResponseBody::DocumentOpen(first_open) = dispatch(
        &service,
        &first_session,
        5,
        RequestBody::DocumentOpen(DocumentOpenRequest {
            workspace_id: first_id,
            path: WorkspaceRelativePath::parse("a.ts").unwrap(),
        }),
    ) else {
        panic!("expected first document");
    };
    let foreign_document = dispatch(
        &service,
        &second_session,
        6,
        RequestBody::DocumentSave(DocumentSaveRequest {
            document_id: first_open.document.document_id,
            base_revision: Revision::new(1),
        }),
    );
    assert!(matches!(
        foreign_document,
        ResponseBody::Error(ref error) if error.code.as_str() == "not_found"
    ));
}

#[test]
fn dirty_document_survives_close_disconnect_and_external_save_conflict() {
    let root = tempfile::tempdir().unwrap();
    let (service, session, workspace_id) = service(root.path(), "workspace-1", "session-1");
    connect(&service, &session, workspace_id.clone(), 1);

    let ResponseBody::DocumentCreate(created) = dispatch(
        &service,
        &session,
        2,
        RequestBody::DocumentCreate(DocumentCreateRequest {
            workspace_id: workspace_id.clone(),
            path: WorkspaceRelativePath::parse("new.ts").unwrap(),
            encoding: DocumentEncoding::Utf8,
            content: "x".to_owned(),
        }),
    ) else {
        panic!("expected created document");
    };
    assert!(created.document.dirty);
    let document_id = created.document.document_id;
    assert!(matches!(
        dispatch(
            &service,
            &session,
            3,
            RequestBody::DocumentSave(DocumentSaveRequest {
                document_id: document_id.clone(),
                base_revision: Revision::new(1),
            }),
        ),
        ResponseBody::DocumentSave(_)
    ));

    let transaction =
        DocumentTransaction::try_new(vec![DocumentTextEdit::try_new(0, 1, "y").unwrap()]).unwrap();
    assert!(matches!(
        dispatch(
            &service,
            &session,
            4,
            RequestBody::DocumentEdit(DocumentEditRequest {
                document_id: document_id.clone(),
                base_revision: Revision::new(1),
                transaction,
            }),
        ),
        ResponseBody::DocumentEdit(_)
    ));
    fs::write(root.path().join("new.ts"), "external").unwrap();
    assert!(matches!(
        dispatch(
            &service,
            &session,
            5,
            RequestBody::DocumentSave(DocumentSaveRequest {
                document_id: document_id.clone(),
                base_revision: Revision::new(2),
            }),
        ),
        ResponseBody::Error(_)
    ));

    let ResponseBody::DocumentUndo(undone) = dispatch(
        &service,
        &session,
        6,
        RequestBody::DocumentUndo(DocumentHistoryRequest {
            document_id: document_id.clone(),
            base_revision: Revision::new(2),
        }),
    ) else {
        panic!("expected undo response");
    };
    assert_eq!(undone.document.content, "x");
    assert_eq!(undone.document.revision, Revision::new(3));
    let ResponseBody::DocumentRedo(redone) = dispatch(
        &service,
        &session,
        7,
        RequestBody::DocumentRedo(DocumentHistoryRequest {
            document_id: document_id.clone(),
            base_revision: Revision::new(3),
        }),
    ) else {
        panic!("expected redo response");
    };
    assert_eq!(redone.document.content, "y");
    assert!(redone.document.dirty);

    assert!(matches!(
        dispatch(
            &service,
            &session,
            8,
            RequestBody::DocumentClose(DocumentCloseRequest { document_id }),
        ),
        ResponseBody::DocumentClose(_)
    ));
    service.disconnect(session.session_id).unwrap();

    let reconnected = BackendSession::new(
        SessionId::try_new("session-2").unwrap(),
        workspace_id.clone(),
    );
    connect(&service, &reconnected, workspace_id.clone(), 9);
    let ResponseBody::SnapshotResync(snapshot) = dispatch(
        &service,
        &reconnected,
        10,
        RequestBody::SnapshotResync(SnapshotRequest {
            workspace_id,
            reason: SnapshotReason::Reconnect,
        }),
    ) else {
        panic!("expected reconnect snapshot");
    };
    assert_eq!(snapshot.snapshot.documents[0].content, "y");
    assert!(snapshot.snapshot.documents[0].dirty);
}

#[test]
fn history_without_a_change_does_not_advance_revision_or_publish_an_event() {
    let root = tempfile::tempdir().unwrap();
    let (service, session, workspace_id) = service(root.path(), "workspace-1", "session-1");
    connect(&service, &session, workspace_id.clone(), 1);
    let events = service.subscribe(session.clone()).unwrap();

    let ResponseBody::DocumentCreate(created) = dispatch(
        &service,
        &session,
        2,
        RequestBody::DocumentCreate(DocumentCreateRequest {
            workspace_id,
            path: WorkspaceRelativePath::parse("new.ts").unwrap(),
            encoding: DocumentEncoding::Utf8,
            content: String::new(),
        }),
    ) else {
        panic!("expected created document");
    };
    events.recv_timeout(Duration::from_secs(1)).unwrap();

    let ResponseBody::DocumentUndo(undone) = dispatch(
        &service,
        &session,
        3,
        RequestBody::DocumentUndo(DocumentHistoryRequest {
            document_id: created.document.document_id,
            base_revision: Revision::new(1),
        }),
    ) else {
        panic!("expected undo response");
    };
    assert_eq!(undone.document.revision, Revision::new(1));
    assert!(events.recv_timeout(Duration::from_millis(20)).is_err());
}

#[test]
fn snapshot_resync_reports_external_change_with_unsent_document_content() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("file.ts"), "before").unwrap();
    let (service, session, workspace_id) = service(root.path(), "workspace-1", "session-1");
    connect(&service, &session, workspace_id.clone(), 1);

    let ResponseBody::DocumentOpen(opened) = dispatch(
        &service,
        &session,
        2,
        RequestBody::DocumentOpen(DocumentOpenRequest {
            workspace_id: workspace_id.clone(),
            path: WorkspaceRelativePath::parse("file.ts").unwrap(),
        }),
    ) else {
        panic!("expected opened document");
    };
    assert_eq!(
        opened.document.external_change,
        DocumentExternalChange::Unchanged
    );

    let transaction =
        DocumentTransaction::try_new(vec![DocumentTextEdit::try_new(6, 6, " local").unwrap()])
            .unwrap();
    assert!(matches!(
        dispatch(
            &service,
            &session,
            3,
            RequestBody::DocumentEdit(DocumentEditRequest {
                document_id: opened.document.document_id,
                base_revision: Revision::new(1),
                transaction,
            }),
        ),
        ResponseBody::DocumentEdit(_)
    ));

    // Host の Document は未保存のまま、実ファイルだけが外部で変わる。
    fs::write(root.path().join("file.ts"), "after").unwrap();

    let ResponseBody::SnapshotResync(snapshot) = dispatch(
        &service,
        &session,
        4,
        RequestBody::SnapshotResync(SnapshotRequest {
            workspace_id,
            reason: SnapshotReason::Reconnect,
        }),
    ) else {
        panic!("expected snapshot");
    };
    let document = &snapshot.snapshot.documents[0];
    assert_eq!(document.content, "before local");
    assert!(document.dirty);
    assert_eq!(document.external_change, DocumentExternalChange::Modified);
}

#[test]
fn snapshot_resync_survives_a_document_parent_replaced_by_a_file() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("src")).unwrap();
    fs::write(root.path().join("src/file.ts"), "broken").unwrap();
    fs::write(root.path().join("healthy.ts"), "healthy").unwrap();
    let (service, session, workspace_id) = service(root.path(), "workspace-1", "session-1");
    connect(&service, &session, workspace_id.clone(), 1);

    for (request, path) in [(2, "src/file.ts"), (3, "healthy.ts")] {
        assert!(matches!(
            dispatch(
                &service,
                &session,
                request,
                RequestBody::DocumentOpen(DocumentOpenRequest {
                    workspace_id: workspace_id.clone(),
                    path: WorkspaceRelativePath::parse(path).unwrap(),
                }),
            ),
            ResponseBody::DocumentOpen(_)
        ));
    }

    // 開いている file の親ディレクトリを通常ファイルへ置き換える。
    fs::remove_file(root.path().join("src/file.ts")).unwrap();
    fs::remove_dir(root.path().join("src")).unwrap();
    fs::write(root.path().join("src"), "not a directory").unwrap();

    let ResponseBody::SnapshotResync(snapshot) = dispatch(
        &service,
        &session,
        4,
        RequestBody::SnapshotResync(SnapshotRequest {
            workspace_id,
            reason: SnapshotReason::Reconnect,
        }),
    ) else {
        panic!("expected snapshot");
    };
    assert_eq!(snapshot.snapshot.documents.len(), 2);
    let broken = snapshot
        .snapshot
        .documents
        .iter()
        .find(|document| {
            document
                .path
                .as_ref()
                .is_some_and(|path| path.as_str() == "src/file.ts")
        })
        .expect("expected broken document");
    let healthy = snapshot
        .snapshot
        .documents
        .iter()
        .find(|document| {
            document
                .path
                .as_ref()
                .is_some_and(|path| path.as_str() == "healthy.ts")
        })
        .expect("expected healthy document");
    assert_eq!(broken.external_change, DocumentExternalChange::Deleted);
    assert_eq!(healthy.external_change, DocumentExternalChange::Unchanged);
}

#[test]
fn snapshot_resync_keeps_an_unsaved_draft_out_of_the_deleted_state() {
    let root = tempfile::tempdir().unwrap();
    let (service, session, workspace_id) = service(root.path(), "workspace-1", "session-1");
    connect(&service, &session, workspace_id.clone(), 1);

    assert!(matches!(
        dispatch(
            &service,
            &session,
            2,
            RequestBody::DocumentCreate(DocumentCreateRequest {
                workspace_id: workspace_id.clone(),
                path: WorkspaceRelativePath::parse("draft.ts").unwrap(),
                encoding: DocumentEncoding::Utf8,
                content: "draft".to_owned(),
            }),
        ),
        ResponseBody::DocumentCreate(_)
    ));

    let ResponseBody::SnapshotResync(snapshot) = dispatch(
        &service,
        &session,
        3,
        RequestBody::SnapshotResync(SnapshotRequest {
            workspace_id,
            reason: SnapshotReason::Reconnect,
        }),
    ) else {
        panic!("expected snapshot");
    };
    let draft = &snapshot.snapshot.documents[0];
    assert!(draft.dirty);
    assert_eq!(draft.external_change, DocumentExternalChange::Unchanged);
}

#[test]
fn reconnect_snapshot_requires_document_inspect_to_reattach() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("file.ts"), "text").unwrap();
    let (service, session, workspace_id) = service(root.path(), "workspace-1", "session-1");
    connect(&service, &session, workspace_id.clone(), 1);

    let ResponseBody::DocumentOpen(opened) = dispatch(
        &service,
        &session,
        2,
        RequestBody::DocumentOpen(DocumentOpenRequest {
            workspace_id: workspace_id.clone(),
            path: WorkspaceRelativePath::parse("file.ts").unwrap(),
        }),
    ) else {
        panic!("expected opened document");
    };
    let document_id = opened.document.document_id;

    assert!(matches!(
        dispatch(
            &service,
            &session,
            3,
            RequestBody::WorkspaceClose(WorkspaceCloseRequest {
                workspace_id: workspace_id.clone(),
            }),
        ),
        ResponseBody::WorkspaceClose(_)
    ));
    connect(&service, &session, workspace_id.clone(), 4);

    let edit = || {
        let transaction =
            DocumentTransaction::try_new(vec![DocumentTextEdit::try_new(4, 4, "!").unwrap()])
                .unwrap();
        RequestBody::DocumentEdit(DocumentEditRequest {
            document_id: document_id.clone(),
            base_revision: Revision::new(1),
            transaction,
        })
    };

    // snapshot.resync は attachment を作らないため、そのままでは編集できない。
    assert!(matches!(
        dispatch(
            &service,
            &session,
            5,
            RequestBody::SnapshotResync(SnapshotRequest {
                workspace_id: workspace_id.clone(),
                reason: SnapshotReason::Reconnect,
            }),
        ),
        ResponseBody::SnapshotResync(_)
    ));
    assert!(matches!(
        dispatch(&service, &session, 6, edit()),
        ResponseBody::Error(ref error) if error.code.as_str() == "forbidden"
    ));

    // document.inspect で再アタッチすると編集できる。
    assert!(matches!(
        dispatch(
            &service,
            &session,
            7,
            RequestBody::DocumentInspect(client_api::DocumentInspectRequest {
                document_id: document_id.clone(),
            }),
        ),
        ResponseBody::DocumentInspect(_)
    ));
    assert!(matches!(
        dispatch(&service, &session, 8, edit()),
        ResponseBody::DocumentEdit(_)
    ));
}

#[test]
fn snapshot_resync_survives_a_document_path_replaced_by_a_directory() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("broken.ts"), "broken").unwrap();
    fs::write(root.path().join("healthy.ts"), "healthy").unwrap();
    let (service, session, workspace_id) = service(root.path(), "workspace-1", "session-1");
    connect(&service, &session, workspace_id.clone(), 1);

    for (request, path) in [(2, "broken.ts"), (3, "healthy.ts")] {
        assert!(matches!(
            dispatch(
                &service,
                &session,
                request,
                RequestBody::DocumentOpen(DocumentOpenRequest {
                    workspace_id: workspace_id.clone(),
                    path: WorkspaceRelativePath::parse(path).unwrap(),
                }),
            ),
            ResponseBody::DocumentOpen(_)
        ));
    }

    // 開いている file をディレクトリへ置き換えると fingerprint の読み取りが失敗する。
    fs::remove_file(root.path().join("broken.ts")).unwrap();
    fs::create_dir(root.path().join("broken.ts")).unwrap();

    let ResponseBody::SnapshotResync(snapshot) = dispatch(
        &service,
        &session,
        4,
        RequestBody::SnapshotResync(SnapshotRequest {
            workspace_id,
            reason: SnapshotReason::Reconnect,
        }),
    ) else {
        panic!("expected snapshot");
    };
    assert_eq!(snapshot.snapshot.documents.len(), 2);
    let broken = snapshot
        .snapshot
        .documents
        .iter()
        .find(|document| {
            document
                .path
                .as_ref()
                .is_some_and(|path| path.as_str() == "broken.ts")
        })
        .expect("expected broken document");
    let healthy = snapshot
        .snapshot
        .documents
        .iter()
        .find(|document| {
            document
                .path
                .as_ref()
                .is_some_and(|path| path.as_str() == "healthy.ts")
        })
        .expect("expected healthy document");
    assert_eq!(broken.external_change, DocumentExternalChange::Deleted);
    assert_eq!(healthy.external_change, DocumentExternalChange::Unchanged);
}

#[test]
fn slow_event_subscriber_is_dropped_without_blocking_backend_state() {
    let root = tempfile::tempdir().unwrap();
    let (service, session, workspace_id) = service(root.path(), "workspace-1", "session-1");
    connect(&service, &session, workspace_id.clone(), 1);
    let events = service.subscribe(session.clone()).unwrap();
    let ResponseBody::DocumentCreate(created) = dispatch(
        &service,
        &session,
        2,
        RequestBody::DocumentCreate(DocumentCreateRequest {
            workspace_id,
            path: WorkspaceRelativePath::parse("busy.ts").unwrap(),
            encoding: DocumentEncoding::Utf8,
            content: "x".to_owned(),
        }),
    ) else {
        panic!("expected created document");
    };

    for index in 0..300_u64 {
        let replacement = if index % 2 == 0 { "y" } else { "x" };
        let transaction = DocumentTransaction::try_new(vec![
            DocumentTextEdit::try_new(0, 1, replacement).unwrap(),
        ])
        .unwrap();
        assert!(matches!(
            dispatch(
                &service,
                &session,
                index + 3,
                RequestBody::DocumentEdit(DocumentEditRequest {
                    document_id: created.document.document_id.clone(),
                    base_revision: Revision::new(index + 1),
                    transaction,
                }),
            ),
            ResponseBody::DocumentEdit(_)
        ));
    }

    let mut received = 0;
    loop {
        match events.recv_timeout(Duration::from_secs(1)) {
            Ok(_) => received += 1,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                panic!("slow subscriber was not disconnected")
            }
        }
    }
    assert!(received > 0 && received < 301);
}

fn service(
    root: &Path,
    workspace: &str,
    session: &str,
) -> (BackendService, BackendSession, WorkspaceId) {
    service_with_timeout(root, workspace, session, None)
}

fn service_with_timeout(
    root: &Path,
    workspace: &str,
    session: &str,
    document_owner_idle_timeout: Option<Duration>,
) -> (BackendService, BackendSession, WorkspaceId) {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<BackendService>();

    let workspace_id = WorkspaceId::try_new(workspace).unwrap();
    let mut registration = WorkspaceRegistration::new(
        workspace_id.clone(),
        "Lapis",
        root.to_owned(),
        Arc::new(TestFiles),
    );
    if let Some(timeout) = document_owner_idle_timeout {
        registration = registration.with_document_owner_idle_timeout(timeout);
    }
    let state = BackendState::new([registration]).unwrap();
    let service = BackendService::start(state).unwrap();
    let session = BackendSession::new(SessionId::try_new(session).unwrap(), workspace_id.clone());
    (service, session, workspace_id)
}

fn connect(
    service: &BackendService,
    session: &BackendSession,
    workspace_id: WorkspaceId,
    request: u64,
) {
    assert!(matches!(
        dispatch(
            service,
            session,
            request,
            RequestBody::WorkspaceConnect(WorkspaceConnectRequest { workspace_id }),
        ),
        ResponseBody::WorkspaceConnect(_)
    ));
}

fn dispatch(
    service: &BackendService,
    session: &BackendSession,
    request: u64,
    body: RequestBody,
) -> ResponseBody {
    service
        .dispatch(
            session.clone(),
            RequestEnvelope {
                request_id: RequestId::try_new(format!("request-{request}")).unwrap(),
                body,
            },
        )
        .unwrap()
        .body
}

fn fingerprint(path: &Path, bytes: &[u8]) -> Result<FileFingerprint, DocumentError> {
    let modified_nanos = fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos());
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    Ok(FileFingerprint::new(
        bytes.len() as u64,
        modified_nanos,
        hasher.finish(),
    ))
}

#[test]
fn workbench_save_finishes_after_client_disconnect_without_blocking_other_requests() {
    use backend_state::{WorkbenchPersistence, WorkbenchStoreResult};
    use std::sync::{Mutex, mpsc};
    use workbench::{ConversationViewState, VersionedViewState};

    struct BlockingStore {
        entered: mpsc::SyncSender<()>,
        release: Mutex<mpsc::Receiver<()>>,
        state: Mutex<VersionedViewState>,
    }
    impl WorkbenchPersistence for BlockingStore {
        fn load(&self) -> std::io::Result<VersionedViewState> {
            Ok(self.state.lock().unwrap().clone())
        }
        fn compare_and_save(
            &self,
            expected: u64,
            view: ConversationViewState,
        ) -> std::io::Result<WorkbenchStoreResult> {
            self.entered.send(()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
            let mut state = self.state.lock().unwrap();
            state.update(expected, view).unwrap();
            Ok(WorkbenchStoreResult::Updated(state.clone()))
        }
    }
    let root = tempfile::tempdir().unwrap();
    let workspace_id = WorkspaceId::try_new("workspace-workbench").unwrap();
    let (entered, wait_entered) = mpsc::sync_channel(1);
    let (release, wait_release) = mpsc::sync_channel(1);
    let store = Arc::new(BlockingStore {
        entered,
        release: Mutex::new(wait_release),
        state: Mutex::new(VersionedViewState::default()),
    });
    let service = BackendService::start(BackendState::new([]).unwrap()).unwrap();
    service
        .register_workspace(
            WorkspaceRegistration::new(
                workspace_id.clone(),
                "test",
                root.path().to_owned(),
                Arc::new(TestFiles),
            )
            .with_workbench_persistence(store),
        )
        .unwrap();
    let writer = BackendSession::new(SessionId::try_new("writer").unwrap(), workspace_id.clone());
    let reader = BackendSession::new(SessionId::try_new("reader").unwrap(), workspace_id.clone());
    connect(&service, &writer, workspace_id.clone(), 1);
    connect(&service, &reader, workspace_id.clone(), 2);
    let writer_service = service.clone();
    let writer_session = writer.clone();
    let writer_workspace = workspace_id.clone();
    let pending = std::thread::spawn(move || {
        dispatch(
            &writer_service,
            &writer_session,
            3,
            RequestBody::WorkbenchPut(client_api::WorkbenchPutRequest {
                workspace_id: writer_workspace,
                expected_revision: 0,
                view: serde_json::json!({"show_panel_controls": false}),
            }),
        )
    });
    wait_entered.recv_timeout(Duration::from_secs(2)).unwrap();
    service.disconnect(writer.session_id.clone()).unwrap();
    let initial = dispatch(
        &service,
        &reader,
        4,
        RequestBody::WorkbenchGet(client_api::WorkbenchGetRequest {
            workspace_id: workspace_id.clone(),
        }),
    );
    assert!(matches!(initial, ResponseBody::WorkbenchGet(snapshot) if snapshot.revision == 0));
    release.send(()).unwrap();
    assert!(matches!(
        pending.join().unwrap(),
        ResponseBody::WorkbenchPut(client_api::WorkbenchPutResponse::Updated(_))
    ));
    let after = dispatch(
        &service,
        &reader,
        5,
        RequestBody::WorkbenchGet(client_api::WorkbenchGetRequest { workspace_id }),
    );
    assert!(
        matches!(after, ResponseBody::WorkbenchGet(snapshot) if snapshot.revision == 1 && snapshot.view["show_panel_controls"] == false)
    );
}

#[test]
fn claim_transfers_the_edit_lease_and_keeps_revision_conflict_detection() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("shared.ts");
    fs::write(&path, "base").unwrap();
    let (service, owner, workspace_id) = service(root.path(), "workspace-lease", "session-owner");
    connect(&service, &owner, workspace_id.clone(), 1);
    let other = BackendSession::new(
        SessionId::try_new("session-other").unwrap(),
        workspace_id.clone(),
    );
    connect(&service, &other, workspace_id.clone(), 2);

    let ResponseBody::DocumentOpen(opened) = dispatch(
        &service,
        &owner,
        3,
        RequestBody::DocumentOpen(DocumentOpenRequest {
            workspace_id: workspace_id.clone(),
            path: WorkspaceRelativePath::parse("shared.ts").unwrap(),
        }),
    ) else {
        panic!("expected opened document");
    };
    let document_id = opened.document.document_id.clone();
    assert!(opened.document.owner.is_none());
    assert!(matches!(
        dispatch(
            &service,
            &other,
            4,
            RequestBody::DocumentInspect(client_api::DocumentInspectRequest {
                document_id: document_id.clone(),
            }),
        ),
        ResponseBody::DocumentInspect(_)
    ));

    let events = service.subscribe(owner.clone()).unwrap();
    assert!(matches!(
        dispatch(
            &service,
            &owner,
            5,
            RequestBody::DocumentClaim(DocumentClaimRequest {
                document_id: document_id.clone(),
            }),
        ),
        ResponseBody::DocumentClaim(_)
    ));
    assert!(matches!(
        events.recv_timeout(Duration::from_secs(1)).unwrap().body,
        EventBody::DocumentOwner { document_id: event_id, owner: Some(owner_id) }
            if event_id == document_id && owner_id == owner.session_id
    ));

    let rejected = dispatch(
        &service,
        &other,
        6,
        RequestBody::DocumentEdit(DocumentEditRequest {
            document_id: document_id.clone(),
            base_revision: Revision::new(1),
            transaction: transaction(0, 4, "other"),
        }),
    );
    assert!(
        matches!(rejected, ResponseBody::Error(ref error) if error.code.as_str() == client_api::INVALID_REQUEST)
    );

    assert!(matches!(
        dispatch(
            &service,
            &other,
            7,
            RequestBody::DocumentClaim(DocumentClaimRequest {
                document_id: document_id.clone(),
            }),
        ),
        ResponseBody::DocumentClaim(_)
    ));
    assert!(matches!(
        events.recv_timeout(Duration::from_secs(1)).unwrap().body,
        EventBody::DocumentOwner { document_id: event_id, owner: Some(owner_id) }
            if event_id == document_id && owner_id == other.session_id
    ));
    assert!(matches!(
        dispatch(
            &service,
            &owner,
            8,
            RequestBody::DocumentEdit(DocumentEditRequest {
                document_id: document_id.clone(),
                base_revision: Revision::new(1),
                transaction: transaction(0, 4, "owner"),
            }),
        ),
        ResponseBody::Error(_)
    ));

    assert!(matches!(
        dispatch(
            &service,
            &other,
            9,
            RequestBody::DocumentEdit(DocumentEditRequest {
                document_id: document_id.clone(),
                base_revision: Revision::new(1),
                transaction: transaction(0, 4, "other"),
            }),
        ),
        ResponseBody::DocumentEdit(_)
    ));
    // 編集権を持っていても古い revision は受理しない。
    assert!(matches!(
        dispatch(
            &service,
            &other,
            10,
            RequestBody::DocumentEdit(DocumentEditRequest {
                document_id,
                base_revision: Revision::new(1),
                transaction: transaction(0, 5, "stale"),
            }),
        ),
        ResponseBody::Error(error) if error.code.as_str() == REVISION_CONFLICT
    ));
}

#[test]
fn disconnect_releases_document_ownership_for_other_sessions() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("shared.ts");
    fs::write(&path, "base").unwrap();
    let (service, owner, workspace_id) =
        service(root.path(), "workspace-disconnect", "session-owner");
    connect(&service, &owner, workspace_id.clone(), 1);
    let other = BackendSession::new(
        SessionId::try_new("session-other").unwrap(),
        workspace_id.clone(),
    );
    connect(&service, &other, workspace_id.clone(), 2);

    let ResponseBody::DocumentOpen(opened) = dispatch(
        &service,
        &owner,
        3,
        RequestBody::DocumentOpen(DocumentOpenRequest {
            workspace_id: workspace_id.clone(),
            path: WorkspaceRelativePath::parse("shared.ts").unwrap(),
        }),
    ) else {
        panic!("expected opened document");
    };
    let document_id = opened.document.document_id.clone();
    dispatch(
        &service,
        &other,
        4,
        RequestBody::DocumentInspect(client_api::DocumentInspectRequest {
            document_id: document_id.clone(),
        }),
    );
    let events = service.subscribe(other.clone()).unwrap();
    assert!(matches!(
        dispatch(
            &service,
            &owner,
            5,
            RequestBody::DocumentClaim(DocumentClaimRequest {
                document_id: document_id.clone(),
            }),
        ),
        ResponseBody::DocumentClaim(_)
    ));
    assert!(matches!(
        events.recv_timeout(Duration::from_secs(1)).unwrap().body,
        EventBody::DocumentOwner { owner: Some(_), .. }
    ));

    service.disconnect(owner.session_id.clone()).unwrap();
    assert!(matches!(
        events.recv_timeout(Duration::from_secs(1)).unwrap().body,
        EventBody::DocumentOwner { document_id: event_id, owner: None }
            if event_id == document_id
    ));

    assert!(matches!(
        dispatch(
            &service,
            &other,
            6,
            RequestBody::DocumentEdit(DocumentEditRequest {
                document_id,
                base_revision: Revision::new(1),
                transaction: transaction(0, 4, "after-disconnect"),
            }),
        ),
        ResponseBody::DocumentEdit(_)
    ));
}

#[test]
fn idle_document_owner_is_released_and_notified() {
    let root = tempfile::tempdir().unwrap();
    let (service, session, workspace_id) = service_with_timeout(
        root.path(),
        "workspace-idle",
        "session-idle",
        Some(Duration::ZERO),
    );
    connect(&service, &session, workspace_id.clone(), 1);
    let ResponseBody::DocumentCreate(created) = dispatch(
        &service,
        &session,
        2,
        RequestBody::DocumentCreate(DocumentCreateRequest {
            workspace_id,
            path: WorkspaceRelativePath::parse("idle.ts").unwrap(),
            encoding: DocumentEncoding::Utf8,
            content: String::new(),
        }),
    ) else {
        panic!("expected created document");
    };
    let document_id = created.document.document_id.clone();
    let events = service.subscribe(session.clone()).unwrap();
    assert!(matches!(
        dispatch(
            &service,
            &session,
            3,
            RequestBody::DocumentClaim(DocumentClaimRequest {
                document_id: document_id.clone(),
            }),
        ),
        ResponseBody::DocumentClaim(_)
    ));
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let event = events.recv_timeout(Duration::from_millis(100)).unwrap();
        if matches!(
            event.body,
            EventBody::DocumentOwner { document_id: event_id, owner: None }
                if event_id == document_id
        ) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "idle release was not notified"
        );
    }
    let ResponseBody::DocumentInspect(snapshot) = dispatch(
        &service,
        &session,
        4,
        RequestBody::DocumentInspect(client_api::DocumentInspectRequest { document_id }),
    ) else {
        panic!("expected inspected document");
    };
    assert!(snapshot.document.owner.is_none());
}

fn transaction(start: u64, end: u64, replacement: &str) -> DocumentTransaction {
    DocumentTransaction::try_new(vec![
        DocumentTextEdit::try_new(start, end, replacement).unwrap(),
    ])
    .unwrap()
}

#[test]
fn failed_workbench_save_does_not_advance_the_host_state() {
    use backend_state::{WorkbenchPersistence, WorkbenchStoreResult};
    use workbench::{ConversationViewState, VersionedViewState};
    struct FailingStore;
    impl WorkbenchPersistence for FailingStore {
        fn load(&self) -> std::io::Result<VersionedViewState> {
            Ok(VersionedViewState::default())
        }
        fn compare_and_save(
            &self,
            _: u64,
            _: ConversationViewState,
        ) -> std::io::Result<WorkbenchStoreResult> {
            Err(std::io::Error::other("disk failure"))
        }
    }
    let root = tempfile::tempdir().unwrap();
    let workspace = WorkspaceId::try_new("workspace-failed-save").unwrap();
    let service = BackendService::start(BackendState::new([]).unwrap()).unwrap();
    service
        .register_workspace(
            WorkspaceRegistration::new(
                workspace.clone(),
                "test",
                root.path().to_owned(),
                Arc::new(TestFiles),
            )
            .with_workbench_persistence(Arc::new(FailingStore)),
        )
        .unwrap();
    let session = BackendSession::new(SessionId::try_new("writer").unwrap(), workspace.clone());
    connect(&service, &session, workspace.clone(), 1);
    let event_receiver = service.subscribe(session.clone()).unwrap();
    let result = service.dispatch(
        session.clone(),
        RequestEnvelope {
            request_id: RequestId::try_new("write-failure").unwrap(),
            body: RequestBody::WorkbenchPut(client_api::WorkbenchPutRequest {
                workspace_id: workspace.clone(),
                expected_revision: 0,
                view: serde_json::json!({"show_panel_controls": false}),
            }),
        },
    );
    assert!(result.is_err());
    assert!(event_receiver.try_recv().is_err());
    let current = dispatch(
        &service,
        &session,
        2,
        RequestBody::WorkbenchGet(client_api::WorkbenchGetRequest {
            workspace_id: workspace,
        }),
    );
    assert!(
        matches!(current, ResponseBody::WorkbenchGet(snapshot) if snapshot.revision == 0 && snapshot.view.is_null())
    );
}
