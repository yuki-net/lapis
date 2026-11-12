//! Workspace view state 共有の Host 側挙動。

use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use backend_state::{
    BackendEventReceiver, BackendService, BackendSession, BackendState, WorkspaceEntry,
    WorkspaceFileBackend, WorkspaceRegistration,
};
use client_api::{
    EventBody, RequestBody, RequestEnvelope, RequestId, ResponseBody, SessionId,
    WorkspaceCloseRequest, WorkspaceConnectRequest, WorkspaceDocumentView, WorkspaceId,
    WorkspaceRelativePath, WorkspaceView, WorkspaceViewSnapshot, WorkspaceViewSubscribeRequest,
    WorkspaceViewUpdateRequest,
};
use document::{DocumentError, DocumentRepository, FileData, FileFingerprint};

#[derive(Default)]
struct EmptyFiles;

impl DocumentRepository for EmptyFiles {
    fn read_file(&self, _path: &Path) -> Result<FileData, DocumentError> {
        Err(DocumentError::io("view state test does not expose files"))
    }

    fn write_file(
        &self,
        _path: &Path,
        _content: &[u8],
        _expected: Option<&FileFingerprint>,
    ) -> Result<FileFingerprint, DocumentError> {
        Err(DocumentError::io("view state test does not expose files"))
    }

    fn fingerprint(&self, _path: &Path) -> Result<Option<FileFingerprint>, DocumentError> {
        Ok(None)
    }
}

impl WorkspaceFileBackend for EmptyFiles {
    fn create_directory(&self, _: &std::path::Path) -> Result<(), DocumentError> {
        Err(DocumentError::io("Unsupported"))
    }
    fn list_children(&self, _directory: &Path) -> Result<Vec<WorkspaceEntry>, DocumentError> {
        Ok(Vec::new())
    }
}

fn service_with_workspace(workspace_id: &WorkspaceId) -> (BackendService, tempfile::TempDir) {
    let root = tempfile::tempdir().unwrap();
    let state = BackendState::new([WorkspaceRegistration::new(
        workspace_id.clone(),
        "View State",
        root.path().to_owned(),
        Arc::new(EmptyFiles),
    )])
    .unwrap();
    (BackendService::start(state).unwrap(), root)
}

fn session(name: &str, workspace_id: &WorkspaceId) -> BackendSession {
    BackendSession::new(SessionId::try_new(name).unwrap(), workspace_id.clone())
}

fn envelope(request_id: &str, body: RequestBody) -> RequestEnvelope {
    RequestEnvelope {
        request_id: RequestId::try_new(request_id).unwrap(),
        body,
    }
}

fn connect(service: &BackendService, session: &BackendSession) {
    let response = service
        .dispatch(
            session.clone(),
            envelope(
                "connect",
                RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
                    workspace_id: session.workspace_id.clone(),
                }),
            ),
        )
        .unwrap();
    assert!(matches!(response.body, ResponseBody::WorkspaceConnect(_)));
}

fn close(service: &BackendService, session: &BackendSession) {
    let response = service
        .dispatch(
            session.clone(),
            envelope(
                "close",
                RequestBody::WorkspaceClose(WorkspaceCloseRequest {
                    workspace_id: session.workspace_id.clone(),
                }),
            ),
        )
        .unwrap();
    assert!(matches!(response.body, ResponseBody::WorkspaceClose(_)));
}

fn subscribe_view(service: &BackendService, session: &BackendSession) -> WorkspaceViewSnapshot {
    let response = service
        .dispatch(
            session.clone(),
            envelope(
                "view-subscribe",
                RequestBody::WorkspaceViewSubscribe(WorkspaceViewSubscribeRequest {
                    workspace_id: session.workspace_id.clone(),
                }),
            ),
        )
        .unwrap();
    match response.body {
        ResponseBody::WorkspaceViewSubscribe(snapshot) => snapshot,
        other => panic!("unexpected response: {other:?}"),
    }
}

fn update_view(service: &BackendService, session: &BackendSession, path: &str) -> u64 {
    let response = service
        .dispatch(
            session.clone(),
            envelope(
                "view-update",
                RequestBody::WorkspaceViewUpdate(WorkspaceViewUpdateRequest {
                    workspace_id: session.workspace_id.clone(),
                    view: view_state(path),
                }),
            ),
        )
        .unwrap();
    match response.body {
        ResponseBody::WorkspaceViewUpdate(response) => response.revision,
        other => panic!("unexpected response: {other:?}"),
    }
}

fn view_state(path: &str) -> WorkspaceView {
    let path = WorkspaceRelativePath::parse(path).unwrap();
    WorkspaceView {
        open_documents: vec![WorkspaceDocumentView {
            document_id: None,
            path: Some(path.clone()),
            cursor_char: 4,
            selection_start: 0,
            selection_end: 0,
            scroll_x: 0.0,
            scroll_y: 12.5,
        }],
        active_path: Some(path),
        active_document_id: None,
    }
}

/// 関係ない event を読み飛ばして view state の delta を待つ。
fn next_view_event(receiver: &BackendEventReceiver) -> (u64, WorkspaceView) {
    loop {
        let event = receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("view state の delta が届く");
        if let EventBody::WorkspaceViewChanged { revision, view, .. } = event.body {
            return (revision, view.expect("参加者がいる間の delta は内容を持つ"));
        }
    }
}

/// 指定時間内に view state の delta が届かないことを確かめる。
fn receives_no_view_event(receiver: &BackendEventReceiver, wait: Duration) -> bool {
    let deadline = Instant::now() + wait;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return true;
        }
        match receiver.recv_timeout(remaining) {
            Ok(event) if matches!(event.body, EventBody::WorkspaceViewChanged { .. }) => {
                return false;
            }
            Ok(_) => {}
            Err(_) => return true,
        }
    }
}

#[test]
fn subscribed_sessions_receive_the_last_view_state_and_its_deltas() {
    let workspace_id = WorkspaceId::try_new("workspace-view").unwrap();
    let (service, _root) = service_with_workspace(&workspace_id);
    let first = session("session-first", &workspace_id);
    let second = session("session-second", &workspace_id);
    connect(&service, &first);
    connect(&service, &second);
    let second_events = service.subscribe(second.clone()).unwrap();

    let initial = subscribe_view(&service, &first);
    assert_eq!(initial.revision, 0);
    assert_eq!(initial.view, None, "共有前は view state を持たない");

    assert_eq!(update_view(&service, &first, "src/main.rs"), 1);

    let (revision, view) = next_view_event(&second_events);
    assert_eq!(revision, 1);
    assert_eq!(
        view.active_path,
        Some(WorkspaceRelativePath::parse("src/main.rs").unwrap())
    );

    // 後から参加した Client は snapshot で追いつき、以後 delta を受ける。
    let joined = subscribe_view(&service, &second);
    assert_eq!(joined.revision, 1);
    let joined_view = joined.view.expect("共有済みの view state");
    assert_eq!(
        joined_view.active_path,
        Some(WorkspaceRelativePath::parse("src/main.rs").unwrap())
    );
}

#[test]
fn closed_sessions_stop_receiving_view_state_deltas() {
    let workspace_id = WorkspaceId::try_new("workspace-view").unwrap();
    let (service, _root) = service_with_workspace(&workspace_id);
    let first = session("session-first", &workspace_id);
    let second = session("session-second", &workspace_id);
    let closed = session("session-closed", &workspace_id);
    for target in [&first, &second, &closed] {
        connect(&service, target);
        subscribe_view(&service, target);
    }
    let second_events = service.subscribe(second.clone()).unwrap();
    let closed_events = service.subscribe(closed.clone()).unwrap();

    assert_eq!(update_view(&service, &first, "src/main.rs"), 1);
    assert_eq!(next_view_event(&closed_events).0, 1);
    next_view_event(&second_events);

    close(&service, &closed);

    assert_eq!(update_view(&service, &first, "src/lib.rs"), 2);
    let (revision, view) = next_view_event(&second_events);
    assert_eq!(revision, 2);
    assert_eq!(
        view.active_path,
        Some(WorkspaceRelativePath::parse("src/lib.rs").unwrap())
    );
    assert!(
        receives_no_view_event(&closed_events, Duration::from_millis(300)),
        "閉じた Session へは delta を配らない"
    );
}

/// `workspace.close` は attachment と購読だけを解除し、Session は閉じない。
/// 同じ Session が `snapshot.resync` と `workspace.connect` で戻れることを保証する。
#[test]
fn closing_a_workspace_keeps_the_session_usable() {
    let workspace_id = WorkspaceId::try_new("workspace-view").unwrap();
    let (service, _root) = service_with_workspace(&workspace_id);
    let session = session("session-close", &workspace_id);
    connect(&service, &session);

    close(&service, &session);

    // close 直後は接続中ではないため、snapshot は拒否される。
    let denied = service
        .dispatch(
            session.clone(),
            envelope(
                "snapshot-after-close",
                RequestBody::SnapshotResync(client_api::SnapshotRequest {
                    workspace_id: workspace_id.clone(),
                    reason: client_api::SnapshotReason::Reconnect,
                }),
            ),
        )
        .unwrap();
    assert!(
        matches!(denied.body, ResponseBody::Error(_)),
        "close 後は接続中ではない"
    );

    // 同じ Session で再接続でき、続けて操作できる。
    connect(&service, &session);
    let snapshot = service
        .dispatch(
            session.clone(),
            envelope(
                "snapshot-after-reconnect",
                RequestBody::SnapshotResync(client_api::SnapshotRequest {
                    workspace_id: workspace_id.clone(),
                    reason: client_api::SnapshotReason::Reconnect,
                }),
            ),
        )
        .unwrap();
    assert!(matches!(snapshot.body, ResponseBody::SnapshotResync(_)));
}
