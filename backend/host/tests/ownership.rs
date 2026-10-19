use lapis_client_api::{
    DocumentEditRequest, DocumentOpenRequest, DocumentTextEdit, DocumentTransaction, RequestBody,
    ResponseBody, SnapshotReason, SnapshotRequest, TerminalStatus, TerminalTerminateRequest,
    WorkspaceRelativePath,
};
use lapis_host::BackendHost;

#[test]
fn registering_the_same_workspace_reuses_its_runtime_identity() {
    let root = tempfile::tempdir().unwrap();
    let host = BackendHost::start().unwrap();
    let first = host.register_workspace(root.path()).unwrap();
    let second = host.register_workspace(&root.path().join(".")).unwrap();
    assert_eq!(first, second);
}

#[test]
fn embedded_clients_share_document_revisions_and_reconnect_state() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("note.txt"), "initial").unwrap();
    let host = BackendHost::start().unwrap();
    let first = host.connect(root.path()).unwrap();
    let second = host.connect(root.path()).unwrap();
    let open = |client: &lapis_backend_state::EmbeddedClient| {
        let ResponseBody::DocumentOpen(response) = client
            .request(RequestBody::DocumentOpen(DocumentOpenRequest {
                workspace_id: client.workspace_id().clone(),
                path: WorkspaceRelativePath::parse("note.txt").unwrap(),
            }))
            .unwrap()
        else {
            panic!("open failed")
        };
        response.document
    };
    let original = open(&first);
    assert_eq!(open(&second), original);
    let transaction =
        DocumentTransaction::try_new(vec![DocumentTextEdit::try_new(0, 7, "updated").unwrap()])
            .unwrap();
    let request = RequestBody::DocumentEdit(DocumentEditRequest {
        document_id: original.document_id.clone(),
        base_revision: original.revision,
        transaction,
    });
    assert!(matches!(
        first.request(request.clone()).unwrap(),
        ResponseBody::DocumentEdit(_)
    ));
    let ResponseBody::Error(conflict) = second.request(request).unwrap() else {
        panic!("stale edit must fail")
    };
    assert_eq!(conflict.code.as_str(), lapis_client_api::REVISION_CONFLICT);
    drop(first);
    drop(second);
    let reconnected = host.connect(root.path()).unwrap();
    let restored = open(&reconnected);
    assert_eq!(restored.content, "updated");
    assert_eq!(restored.document_id, original.document_id);
    assert!(restored.revision > original.revision);
}

#[test]
fn dropping_desktop_terminal_attachment_does_not_terminate_host_process() {
    let root = tempfile::tempdir().unwrap();
    let host = BackendHost::start().unwrap();
    host.register_workspace(root.path()).unwrap();
    let child = root.path().join("nested");
    std::fs::create_dir(&child).unwrap();
    let terminals = host.terminals();
    let terminal_id = terminals.start(&child, 80, 24).unwrap();
    let remote = host.connect(root.path()).unwrap();
    let snapshot = || {
        let ResponseBody::SnapshotResync(response) = remote
            .request(RequestBody::SnapshotResync(SnapshotRequest {
                workspace_id: remote.workspace_id().clone(),
                reason: SnapshotReason::Reconnect,
            }))
            .unwrap()
        else {
            panic!("snapshot failed")
        };
        response.snapshot
    };
    assert_eq!(
        snapshot().terminals[0].terminal_id.as_str(),
        terminal_id.as_str()
    );
    drop(terminals);
    let retained = snapshot();
    assert_eq!(retained.terminals[0].status, TerminalStatus::Running);
    assert!(matches!(
        remote
            .request(RequestBody::TerminalTerminate(TerminalTerminateRequest {
                terminal_id: retained.terminals[0].terminal_id.clone(),
            }))
            .unwrap(),
        ResponseBody::TerminalTerminate(_)
    ));
    assert_eq!(snapshot().terminals[0].status, TerminalStatus::Terminated);
}
