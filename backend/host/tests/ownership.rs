use lapis_client_api::{
    DocumentEditRequest, DocumentOpenRequest, DocumentTextEdit, DocumentTransaction, EventBody,
    RequestBody, ResponseBody, SnapshotReason, SnapshotRequest, TerminalStatus,
    TerminalTerminateRequest, WorkspaceRelativePath,
};
use lapis_host::BackendHost;

#[test]
fn persisted_workbench_survives_host_restart() {
    use lapis_client_api::{WorkbenchGetRequest, WorkbenchPutRequest, WorkbenchPutResponse};
    let root = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let saved = {
        let host = BackendHost::start_with_state_directory(state.path().to_owned()).unwrap();
        let client = host.connect(root.path()).unwrap();
        let result = client
            .request(RequestBody::WorkbenchPut(WorkbenchPutRequest {
                workspace_id: client.workspace_id().clone(),
                expected_revision: 0,
                view: serde_json::json!({"show_panel_controls": false}),
            }))
            .unwrap();
        let ResponseBody::WorkbenchPut(WorkbenchPutResponse::Updated(snapshot)) = result else {
            panic!("workbench save")
        };
        snapshot
    };
    let restarted = BackendHost::start_with_state_directory(state.path().to_owned()).unwrap();
    let client = restarted.connect(root.path()).unwrap();
    let ResponseBody::WorkbenchGet(restored) = client
        .request(RequestBody::WorkbenchGet(WorkbenchGetRequest {
            workspace_id: client.workspace_id().clone(),
        }))
        .unwrap()
    else {
        panic!("workbench restore")
    };
    assert_eq!(restored, saved);
}

#[test]
fn concurrent_workbench_writers_cannot_silently_overwrite_each_other() {
    use lapis_client_api::{WorkbenchGetRequest, WorkbenchPutRequest, WorkbenchPutResponse};
    use std::sync::{Arc, Barrier};

    let root = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let host = BackendHost::start_with_state_directory(state.path().to_owned()).unwrap();
    let first = host.connect(root.path()).unwrap();
    let second = host.connect(root.path()).unwrap();
    let workspace_id = first.workspace_id().clone();
    let barrier = Arc::new(Barrier::new(3));
    let spawn = |client: lapis_backend_state::EmbeddedClient, label: &str| {
        let barrier = barrier.clone();
        let workspace_id = workspace_id.clone();
        let label = label.to_owned();
        std::thread::spawn(move || {
            barrier.wait();
            client.request(RequestBody::WorkbenchPut(WorkbenchPutRequest {
                workspace_id,
                expected_revision: 0,
                view: serde_json::json!({"workbench": {"bindings": [{"view": label, "scope": "shared-workbench"}]}}),
            })).unwrap()
        })
    };
    let a = spawn(first, "first");
    let b = spawn(second, "second");
    barrier.wait();
    let responses = [a.join().unwrap(), b.join().unwrap()];
    let accepted = responses
        .iter()
        .filter_map(|response| match response {
            ResponseBody::WorkbenchPut(WorkbenchPutResponse::Updated(snapshot)) => Some(snapshot),
            _ => None,
        })
        .collect::<Vec<_>>();
    let rejected = responses
        .iter()
        .filter_map(|response| match response {
            ResponseBody::WorkbenchPut(WorkbenchPutResponse::Conflict(snapshot)) => Some(snapshot),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(accepted.len(), 1);
    assert_eq!(rejected, accepted);
    let observer = host.connect(root.path()).unwrap();
    let ResponseBody::WorkbenchGet(current) = observer
        .request(RequestBody::WorkbenchGet(WorkbenchGetRequest {
            workspace_id,
        }))
        .unwrap()
    else {
        panic!("workbench get")
    };
    assert_eq!(&current, accepted[0]);
}

#[test]
fn clients_share_workbench_updates_and_reject_stale_writes() {
    use lapis_client_api::{
        EventBody, WorkbenchGetRequest, WorkbenchPutRequest, WorkbenchPutResponse,
    };
    use std::time::Duration;

    let root = tempfile::tempdir().unwrap();
    let host = BackendHost::start().unwrap();
    let first = host.connect(root.path()).unwrap();
    let second = host.connect(root.path()).unwrap();
    let workspace_id = first.workspace_id().clone();
    let events = second.subscribe().unwrap();
    let ResponseBody::WorkbenchGet(initial) = first
        .request(RequestBody::WorkbenchGet(WorkbenchGetRequest {
            workspace_id: workspace_id.clone(),
        }))
        .unwrap()
    else {
        panic!("workbench get")
    };
    assert!(initial.view.is_null());
    let view = serde_json::json!({"show_panel_controls": false});
    let request = WorkbenchPutRequest {
        workspace_id: workspace_id.clone(),
        expected_revision: initial.revision,
        view: view.clone(),
    };
    let ResponseBody::WorkbenchPut(WorkbenchPutResponse::Updated(updated)) = first
        .request(RequestBody::WorkbenchPut(request.clone()))
        .unwrap()
    else {
        panic!("workbench put")
    };
    assert_eq!(updated.revision, initial.revision + 1);
    assert_eq!(updated.view["show_panel_controls"], false);
    assert!(
        matches!(events.recv_timeout(Duration::from_secs(2)).unwrap().body, EventBody::WorkbenchChanged { revision, .. } if revision == updated.revision)
    );
    let ResponseBody::WorkbenchPut(WorkbenchPutResponse::Conflict(current)) =
        second.request(RequestBody::WorkbenchPut(request)).unwrap()
    else {
        panic!("stale update")
    };
    assert_eq!(current, updated);
    drop(first);
    drop(second);
    let reconnected = host.connect(root.path()).unwrap();
    let ResponseBody::WorkbenchGet(restored) = reconnected
        .request(RequestBody::WorkbenchGet(WorkbenchGetRequest {
            workspace_id,
        }))
        .unwrap()
    else {
        panic!("workbench restore")
    };
    assert_eq!(restored, updated);
}

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
    let first_events = first.subscribe().unwrap();
    let second_events = second.subscribe().unwrap();
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
    let first_event = first_events
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    let second_event = second_events
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    assert_eq!(first_event, second_event);
    let EventBody::DocumentEdited { revision, .. } = second_event.body else {
        panic!("expected first edit event")
    };
    let ResponseBody::Error(conflict) = second.request(request).unwrap() else {
        panic!("stale edit must fail")
    };
    assert_eq!(conflict.code.as_str(), lapis_client_api::REVISION_CONFLICT);
    let second_edit = RequestBody::DocumentEdit(DocumentEditRequest {
        document_id: original.document_id.clone(),
        base_revision: revision,
        transaction: DocumentTransaction::try_new(vec![
            DocumentTextEdit::try_new(7, 7, "!").unwrap(),
        ])
        .unwrap(),
    });
    assert!(matches!(
        second.request(second_edit).unwrap(),
        ResponseBody::DocumentEdit(_)
    ));
    let first_event = first_events
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    let second_event = second_events
        .recv_timeout(std::time::Duration::from_secs(2))
        .unwrap();
    assert_eq!(first_event, second_event);
    assert!(
        matches!(first_event.body, EventBody::DocumentEdited { base_revision, .. } if base_revision == revision)
    );
    drop(first);
    drop(second);
    let reconnected = host.connect(root.path()).unwrap();
    let restored = open(&reconnected);
    assert_eq!(restored.content, "updated!");
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
