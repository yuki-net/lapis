use std::sync::Arc;

use lapis_backend_state::{BackendService, BackendSession, BackendState, WorkspaceRegistration};
use lapis_client_api::{
    DocumentEditRequest, DocumentOpenRequest, DocumentTextEdit, DocumentTransaction, RequestBody,
    RequestEnvelope, RequestId, ResponseBody, SessionId, WorkspaceConnectRequest, WorkspaceId,
    WorkspaceRelativePath,
};
use lapis_document::DocumentRepository;

use lapis_platform::{BackendClient, LocalTerminalBackend, LocalWorkspaceRepository};

#[test]
fn local_client_and_remote_session_observe_one_backend_state() {
    let workspace = tempfile::tempdir().unwrap();
    let path = workspace.path().join("notes.md");
    std::fs::write(&path, "initial").unwrap();

    let files = Arc::new(LocalWorkspaceRepository);
    let terminal = Arc::new(LocalTerminalBackend::default());
    let workspace_id = WorkspaceId::try_new("workspace-1").unwrap();
    let state = BackendState::new([WorkspaceRegistration::new(
        workspace_id.clone(),
        "Workspace",
        workspace.path().to_owned(),
        files.clone(),
    )
    .with_terminal(terminal)])
    .unwrap();
    let service = BackendService::start(state).unwrap();
    let local = BackendClient::connect(
        service.clone(),
        SessionId::try_new("desktop-local").unwrap(),
        workspace_id.clone(),
        workspace.path().to_owned(),
        files.clone(),
    )
    .unwrap();
    let repository = local.workspace_repository();

    let initial = repository.read_file(&path).unwrap();
    let local_fingerprint = initial.fingerprint.clone();
    repository
        .write_file(&path, b"from-local", Some(&local_fingerprint))
        .unwrap();

    let remote_session = BackendSession::new(
        SessionId::try_new("remote-session").unwrap(),
        workspace_id.clone(),
    );
    dispatch(
        &service,
        &remote_session,
        RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
            workspace_id: workspace_id.clone(),
        }),
        1,
    );
    let opened = dispatch(
        &service,
        &remote_session,
        RequestBody::DocumentOpen(DocumentOpenRequest {
            workspace_id: workspace_id.clone(),
            path: WorkspaceRelativePath::parse("notes.md").unwrap(),
        }),
        2,
    );
    let ResponseBody::DocumentOpen(opened) = opened else {
        panic!("remote session must open the document from the shared state");
    };
    assert_eq!(opened.document.content, "from-local");

    let edit = DocumentTextEdit::try_new(
        0,
        opened.document.content.chars().count() as u64,
        "from-remote",
    )
    .unwrap();
    let edited = dispatch(
        &service,
        &remote_session,
        RequestBody::DocumentEdit(DocumentEditRequest {
            document_id: opened.document.document_id,
            base_revision: opened.document.revision,
            transaction: DocumentTransaction::try_new(vec![edit]).unwrap(),
        }),
        3,
    );
    assert!(matches!(edited, ResponseBody::DocumentEdit(_)));

    assert_eq!(repository.read_file(&path).unwrap().bytes, b"from-remote");
    service.disconnect(remote_session.session_id).unwrap();
    assert_eq!(repository.read_file(&path).unwrap().bytes, b"from-remote");
    local.disconnect().unwrap();
}

fn dispatch(
    service: &BackendService,
    session: &BackendSession,
    body: RequestBody,
    number: u64,
) -> ResponseBody {
    service
        .dispatch(
            session.clone(),
            RequestEnvelope {
                request_id: RequestId::try_new(format!("request-{number}")).unwrap(),
                body,
            },
        )
        .unwrap()
        .body
}
