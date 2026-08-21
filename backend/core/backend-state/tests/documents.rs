use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::Path,
    sync::Arc,
    time::Duration,
};

use lapis_backend_state::{
    BackendService, BackendSession, BackendState, WorkspaceEntry, WorkspaceEntryKind,
    WorkspaceFileBackend, WorkspaceRegistration,
};
use lapis_client_api::{
    DocumentCloseRequest, DocumentCreateRequest, DocumentEditRequest, DocumentEncoding,
    DocumentHistoryRequest, DocumentOpenRequest, DocumentSaveRequest, DocumentTextEdit,
    DocumentTransaction, EventBody, FileTreeRequest, REVISION_CONFLICT, RequestBody,
    RequestEnvelope, RequestId, ResponseBody, Revision, SessionId, SnapshotReason, SnapshotRequest,
    WorkspaceCloseRequest, WorkspaceConnectRequest, WorkspaceId, WorkspaceRelativePath,
};
use lapis_document::{DocumentError, DocumentRepository, FileData, FileFingerprint};

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
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<BackendService>();

    let workspace_id = WorkspaceId::try_new(workspace).unwrap();
    let state = BackendState::new([WorkspaceRegistration::new(
        workspace_id.clone(),
        "Lapis",
        root.to_owned(),
        Arc::new(TestFiles),
    )])
    .unwrap();
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
