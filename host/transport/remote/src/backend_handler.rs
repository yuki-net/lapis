use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use backend_state::{BackendEventSink, BackendService, BackendSession};
use client_api::{
    ErrorCode, EventEnvelope, INTERNAL, ProtocolError, RequestBody, RequestEnvelope, ResponseBody,
    ResponseEnvelope, SessionId,
};

use crate::{
    RemoteEventReceiver, RemoteRequestHandler, RemoteResponseFuture, RemoteSubscriptionError,
    RemoteSubscriptionFuture, SessionGrant,
};

const REMOTE_EVENT_QUEUE_CAPACITY: usize = 64;

#[derive(Clone)]
pub struct BackendRemoteHandler {
    backend: BackendService,
    pending_subscriptions: Arc<Mutex<HashMap<SessionId, RemoteEventReceiver>>>,
}

impl BackendRemoteHandler {
    pub fn new(backend: BackendService) -> Self {
        Self {
            backend,
            pending_subscriptions: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl RemoteRequestHandler for BackendRemoteHandler {
    fn handle(&self, grant: SessionGrant, request: RequestEnvelope) -> RemoteResponseFuture<'_> {
        let backend = self.backend.clone();
        let cleanup_backend = self.backend.clone();
        let request_id = request.request_id.clone();
        // `workspace.switch` は Workspace 一覧・登録と、同一接続での Workspace 切替を許可する。
        let may_switch = grant
            .capabilities()
            .iter()
            .any(|capability| capability.as_str() == client_api::capability::WORKSPACES_SWITCH);
        // Workspace へ接続する要求だけは、切替権限を持つ Session に限り要求先を
        // Session の接続先として扱う。接続中の Workspace かどうかは Runtime が判定する。
        let session_workspace = match (&request.body, may_switch) {
            (RequestBody::WorkspaceConnect(connect), true) => connect.workspace_id.clone(),
            _ => grant.workspace_id().clone(),
        };
        let session = BackendSession::new(grant.session_id().clone(), session_workspace)
            .with_user_workspace_listing(may_switch);
        let opens_workspace = matches!(&request.body, RequestBody::WorkspaceConnect(_));
        let can_read_git = grant.capabilities().contains(
            &client_api::CapabilityId::try_new("workspace.git.read").expect("valid capability"),
        );
        // view state 共有の購読と同じく、読取りには workspace.sync を要求する。
        let can_read_view = grant.capabilities().contains(
            &client_api::CapabilityId::try_new(client_api::capability::WORKSPACE_SYNC)
                .expect("valid capability"),
        );
        let pending_subscriptions = self.pending_subscriptions.clone();
        Box::pin(async move {
            let receiver = opens_workspace.then(|| {
                let (sender, receiver) = tokio::sync::mpsc::channel(REMOTE_EVENT_QUEUE_CAPACITY);
                (
                    Arc::new(TokioEventSink {
                        sender,
                        can_read_git,
                        can_read_view,
                    }) as Arc<dyn BackendEventSink>,
                    receiver,
                )
            });
            let sink = receiver.as_ref().map(|(sink, _)| sink.clone());
            let dispatched = tokio::task::spawn_blocking(move || match sink {
                Some(sink) => backend.dispatch_with_subscription(session, request, sink),
                None => backend.dispatch(session, request),
            })
            .await;
            match dispatched {
                Ok(Ok(mut response)) => {
                    if !can_read_git
                        && let ResponseBody::SnapshotResync(snapshot) = &mut response.body
                    {
                        snapshot.snapshot.git_repositories.clear();
                    }
                    if matches!(&response.body, ResponseBody::WorkspaceConnect(_))
                        && let Some((_, receiver)) = receiver
                    {
                        let session_id = grant.session_id().clone();
                        if let Ok(mut pending) = pending_subscriptions.lock() {
                            pending.insert(session_id, RemoteEventReceiver::new(receiver));
                        } else {
                            let _ = cleanup_backend.disconnect(session_id);
                            return internal_error(request_id);
                        }
                    }
                    response
                }
                Ok(Err(_)) | Err(_) => internal_error(request_id),
            }
        })
    }

    fn disconnect(&self, grant: &SessionGrant) {
        if let Ok(mut pending) = self.pending_subscriptions.lock() {
            pending.remove(grant.session_id());
        }
        let _ = self.backend.disconnect(grant.session_id().clone());
    }

    fn subscribe(&self, grant: SessionGrant) -> RemoteSubscriptionFuture<'_> {
        let receiver = self
            .pending_subscriptions
            .lock()
            .map_err(|_| RemoteSubscriptionError)
            .and_then(|mut pending| {
                pending
                    .remove(grant.session_id())
                    .ok_or(RemoteSubscriptionError)
            });
        Box::pin(async move { receiver.map(Some) })
    }
}

struct TokioEventSink {
    sender: tokio::sync::mpsc::Sender<EventEnvelope>,
    can_read_git: bool,
    can_read_view: bool,
}

impl BackendEventSink for TokioEventSink {
    fn try_send(&self, mut event: EventEnvelope) -> bool {
        if !self.can_read_git
            && let client_api::EventBody::GitStatusChanged { workspace_id, .. }
            | client_api::EventBody::GitRepositoriesChanged { workspace_id }
            | client_api::EventBody::GitWorktreesChanged { workspace_id } = &event.body
        {
            event.body = client_api::EventBody::WorkspaceChanged {
                workspace_id: workspace_id.clone(),
            };
        }
        // 配信自体を止めると event 連番が欠番になり Client が切断するため、
        // 権限が無い Session にも連番を保ったまま内容だけを伏せた event を送る。
        if !self.can_read_view
            && let client_api::EventBody::WorkspaceViewChanged {
                workspace_id,
                revision,
                ..
            } = &event.body
        {
            event.body = client_api::EventBody::WorkspaceViewChanged {
                workspace_id: workspace_id.clone(),
                revision: *revision,
                view: None,
            };
        }
        self.sender.try_send(event).is_ok()
    }
}

fn internal_error(request_id: client_api::RequestId) -> ResponseEnvelope {
    ResponseEnvelope {
        request_id,
        body: ResponseBody::Error(ProtocolError::new(
            ErrorCode::try_new(INTERNAL).expect("built-in error code must be valid"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::{path::Path, sync::Arc};

    use backend_state::{
        BackendState, WorkspaceEntry, WorkspaceFileBackend, WorkspaceRegistration,
    };
    use client_api::{
        CapabilityId, WorkspaceOperation as Operation, WorkspaceOperationRequest,
        WorkspaceOperationResponse as Outcome,
    };
    use client_api::{
        CapabilitySet, DocumentCreateRequest, DocumentEncoding, EventBody, RequestBody, RequestId,
        ResponseBody, SessionId, SnapshotReason, SnapshotRequest, WorkspaceConnectRequest,
        WorkspaceDocumentView, WorkspaceId, WorkspaceRelativePath, WorkspaceView,
        WorkspaceViewSubscribeRequest, WorkspaceViewUpdateRequest,
    };
    use document::{DocumentError, DocumentRepository, FileData, FileFingerprint};
    use editor_core::{ConversationId, ExecutionId, TaskId};
    use task::{
        ExecutionStatus, TaskBackend, TaskControl, TaskError, TaskMode, TaskRecord, TaskStart,
    };

    use super::*;

    struct EmptyFiles;

    impl DocumentRepository for EmptyFiles {
        fn read_file(&self, _path: &Path) -> Result<FileData, DocumentError> {
            Err(DocumentError::io("files are not used by this test"))
        }

        fn write_file(
            &self,
            _path: &Path,
            _content: &[u8],
            _expected: Option<&FileFingerprint>,
        ) -> Result<FileFingerprint, DocumentError> {
            Err(DocumentError::io("files are not used by this test"))
        }

        fn fingerprint(&self, _path: &Path) -> Result<Option<FileFingerprint>, DocumentError> {
            Ok(None)
        }
    }

    impl WorkspaceFileBackend for EmptyFiles {
        fn create_directory(&self, _: &std::path::Path) -> Result<(), document::DocumentError> {
            Err(document::DocumentError::io("Unsupported"))
        }
        fn list_children(&self, _directory: &Path) -> Result<Vec<WorkspaceEntry>, DocumentError> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn dispatches_granted_session_and_disconnects_backend_state() {
        let root = tempfile::tempdir().unwrap();
        let workspace_id = WorkspaceId::try_new("workspace-1").unwrap();
        let state = BackendState::new([WorkspaceRegistration::new(
            workspace_id.clone(),
            "Workspace",
            root.path().to_owned(),
            Arc::new(EmptyFiles),
        )])
        .unwrap();
        let handler = BackendRemoteHandler::new(BackendService::start(state).unwrap());
        let grant = SessionGrant::from_authenticated(
            SessionId::try_new("session-1").unwrap(),
            workspace_id.clone(),
            CapabilitySet::default(),
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let connected = runtime.block_on(handler.handle(
            grant.clone(),
            request(
                1,
                RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
                    workspace_id: workspace_id.clone(),
                }),
            ),
        ));
        assert!(matches!(connected.body, ResponseBody::WorkspaceConnect(_)));

        let created = runtime.block_on(handler.handle(
            grant.clone(),
            request(
                4,
                RequestBody::DocumentCreate(DocumentCreateRequest {
                    workspace_id: workspace_id.clone(),
                    path: WorkspaceRelativePath::parse("new.ts").unwrap(),
                    encoding: DocumentEncoding::Utf8,
                    content: "const value = 1;".to_owned(),
                }),
            ),
        ));
        assert!(matches!(created.body, ResponseBody::DocumentCreate(_)));
        let mut events = runtime
            .block_on(handler.subscribe(grant.clone()))
            .unwrap()
            .expect("backend handler must expose events");
        let event = runtime
            .block_on(async {
                tokio::time::timeout(std::time::Duration::from_secs(1), events.recv()).await
            })
            .unwrap()
            .unwrap();
        assert!(matches!(event.body, EventBody::DocumentReplaced { .. }));

        let snapshot = runtime.block_on(handler.handle(
            grant.clone(),
            request(
                2,
                RequestBody::SnapshotResync(SnapshotRequest {
                    workspace_id: workspace_id.clone(),
                    reason: SnapshotReason::Reconnect,
                }),
            ),
        ));
        assert!(matches!(snapshot.body, ResponseBody::SnapshotResync(_)));

        handler.disconnect(&grant);
        let disconnected = runtime.block_on(handler.handle(
            grant,
            request(
                3,
                RequestBody::SnapshotResync(SnapshotRequest {
                    workspace_id,
                    reason: SnapshotReason::Reconnect,
                }),
            ),
        ));
        assert!(matches!(disconnected.body, ResponseBody::Error(_)));
    }

    #[derive(Default)]
    struct FakeTasks {
        records: Mutex<Vec<TaskRecord>>,
        controls: Mutex<Vec<(ExecutionId, TaskControl)>>,
    }

    impl TaskBackend for FakeTasks {
        fn load(&self) -> Result<Vec<TaskRecord>, TaskError> {
            Ok(self.records.lock().unwrap().clone())
        }

        fn start(&self, record: &TaskRecord) -> Result<(), TaskError> {
            self.records.lock().unwrap().push(record.clone());
            Ok(())
        }

        fn control(&self, id: &ExecutionId, control: &TaskControl) -> Result<(), TaskError> {
            self.controls
                .lock()
                .unwrap()
                .push((id.clone(), control.clone()));
            Ok(())
        }
    }

    fn task_input() -> TaskStart {
        TaskStart {
            prompt: "契約テスト".into(),
            mode: TaskMode::Plan,
            isolated: false,
            conversation_id: ConversationId::new("conversation-1"),
        }
    }

    fn operation(workspace_id: &WorkspaceId, operation: Operation) -> RequestBody {
        RequestBody::WorkspaceExecute(WorkspaceOperationRequest {
            workspace_id: workspace_id.clone(),
            operation,
        })
    }

    fn grant(capabilities: &[&str]) -> SessionGrant {
        SessionGrant::from_authenticated(
            SessionId::try_new("remote-1").unwrap(),
            WorkspaceId::try_new("workspace-1").unwrap(),
            CapabilitySet::try_new(
                capabilities
                    .iter()
                    .map(|value| CapabilityId::try_new(*value).unwrap()),
            )
            .unwrap(),
        )
    }

    #[test]
    fn workspace_execute_grants_separate_every_git_and_task_capability() {
        let workspace_id = WorkspaceId::try_new("workspace-1").unwrap();
        let task_id = TaskId::new("task-1");
        let path = WorkspaceRelativePath::parse("note.txt").unwrap();
        let capabilities = [
            "workspace.git.read",
            "workspace.git.write",
            "workspace.tasks.read",
            "workspace.tasks.start",
            "workspace.tasks.control",
        ];
        let operations = [
            (Operation::GitStatus { worktree: None }, capabilities[0]),
            (
                Operation::GitDiff {
                    path: path.clone(),
                    worktree: None,
                },
                capabilities[0],
            ),
            (Operation::Worktrees, capabilities[0]),
            (Operation::CreateWorktree, capabilities[1]),
            (
                Operation::ImportFile {
                    worktree_id: task_id.clone(),
                    path,
                },
                capabilities[1],
            ),
            (
                Operation::DiscardWorktree {
                    worktree_id: task_id,
                },
                capabilities[1],
            ),
            (Operation::Tasks, capabilities[2]),
            (
                Operation::StartTask {
                    input: task_input(),
                },
                capabilities[3],
            ),
            (
                Operation::ControlTask {
                    execution_id: ExecutionId::new("execution-1"),
                    control: TaskControl::Cancel,
                },
                capabilities[4],
            ),
        ];
        for (command, expected) in operations {
            let request = operation(&workspace_id, command);
            assert_eq!(
                grant(&[]).require_request(&request),
                Err(crate::AuthorizationError::CapabilityDenied(expected.into()))
            );
            for capability in capabilities {
                let result = grant(&[capability]).require_request(&request);
                if capability == expected {
                    assert_eq!(result, Ok(()));
                } else {
                    assert_eq!(
                        result,
                        Err(crate::AuthorizationError::CapabilityDenied(expected.into()))
                    );
                }
            }
        }
    }

    #[test]
    fn remote_and_embedded_clients_share_task_state_after_workspace_connect() {
        let root = tempfile::tempdir().unwrap();
        let workspace_id = WorkspaceId::try_new("workspace-1").unwrap();
        let tasks = Arc::new(FakeTasks::default());
        let state = BackendState::new([WorkspaceRegistration::new(
            workspace_id.clone(),
            "Workspace",
            root.path().to_owned(),
            Arc::new(EmptyFiles),
        )
        .with_tasks(tasks.clone())])
        .unwrap();
        let backend = BackendService::start(state).unwrap();
        let embedded = backend_state::EmbeddedClient::connect(
            backend.clone(),
            SessionId::try_new("embedded-1").unwrap(),
            workspace_id.clone(),
        )
        .unwrap();
        let handler = BackendRemoteHandler::new(backend);
        let grant = grant(&[
            "workspace.connect",
            "workspace.tasks.read",
            "workspace.tasks.start",
            "workspace.tasks.control",
        ]);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let dispatch = |number, body: RequestBody| {
            grant.require_request(&body).unwrap();
            runtime
                .block_on(handler.handle(grant.clone(), request(number, body)))
                .body
        };
        assert!(matches!(
            dispatch(
                1,
                RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
                    workspace_id: workspace_id.clone()
                })
            ),
            ResponseBody::WorkspaceConnect(_)
        ));
        let ResponseBody::WorkspaceExecute(Outcome::TaskStarted(remote_record)) = dispatch(
            2,
            operation(
                &workspace_id,
                Operation::StartTask {
                    input: task_input(),
                },
            ),
        ) else {
            panic!("Remote 開始結果が必要")
        };
        let remote_record = *remote_record;
        assert_eq!(
            remote_record.execution.workspace_id.as_str(),
            workspace_id.as_str()
        );
        assert_eq!(remote_record.execution.status, ExecutionStatus::Queued);
        let list = operation(&workspace_id, Operation::Tasks);
        assert_eq!(
            embedded.request(list.clone()).unwrap(),
            ResponseBody::WorkspaceExecute(Outcome::Tasks(vec![remote_record.clone()]))
        );
        let ResponseBody::WorkspaceExecute(Outcome::TaskStarted(embedded_record)) = embedded
            .request(operation(
                &workspace_id,
                Operation::StartTask {
                    input: task_input(),
                },
            ))
            .unwrap()
        else {
            panic!("Embedded 開始結果が必要")
        };
        let embedded_record = *embedded_record;
        assert_ne!(embedded_record.task.id, remote_record.task.id);
        assert_ne!(embedded_record.execution.id, remote_record.execution.id);
        assert_eq!(
            embedded_record.execution.workspace_id.as_str(),
            workspace_id.as_str()
        );
        let expected = ResponseBody::WorkspaceExecute(Outcome::Tasks(vec![
            remote_record.clone(),
            embedded_record,
        ]));
        assert_eq!(dispatch(3, list.clone()), expected);
        assert_eq!(embedded.request(list.clone()).unwrap(), expected);
        assert_eq!(
            dispatch(
                4,
                operation(
                    &workspace_id,
                    Operation::ControlTask {
                        execution_id: remote_record.execution.id.clone(),
                        control: TaskControl::Cancel
                    }
                )
            ),
            ResponseBody::WorkspaceExecute(Outcome::Completed)
        );
        assert_eq!(
            *tasks.controls.lock().unwrap(),
            vec![(remote_record.execution.id, TaskControl::Cancel)]
        );
        handler.disconnect(&grant);
        assert_eq!(embedded.request(list.clone()).unwrap(), expected);
        assert!(matches!(
            dispatch(
                5,
                RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
                    workspace_id: workspace_id.clone()
                })
            ),
            ResponseBody::WorkspaceConnect(_)
        ));
        assert_eq!(dispatch(6, list), expected);
        assert_eq!(tasks.load().unwrap().len(), 2);
        assert_eq!(tasks.controls.lock().unwrap().len(), 1);
        handler.disconnect(&grant);
    }

    #[test]
    fn git_events_are_redacted_without_git_read_capability() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
        let sink = TokioEventSink {
            sender,
            can_read_git: false,
            can_read_view: true,
        };
        let workspace_id = WorkspaceId::try_new("workspace").unwrap();
        let event = EventEnvelope {
            event_sequence: client_api::EventSequence::new(7),
            body: EventBody::GitStatusChanged {
                workspace_id: workspace_id.clone(),
                repository: git::GitRepositoryStatus {
                    repository_id: "path:private".into(),
                    status: git::GitSnapshot {
                        workspace_id: editor_core::WorkspaceId::new("workspace"),
                        worktree_id: None,
                        revision: 1,
                        branch: "secret".into(),
                        head: String::new(),
                        files: vec![],
                    },
                },
            },
        };
        assert!(sink.try_send(event));
        let received = receiver.try_recv().unwrap();
        assert_eq!(received.event_sequence.value(), 7);
        assert!(
            matches!(received.body, EventBody::WorkspaceChanged { workspace_id: id } if id == workspace_id)
        );
    }

    #[test]
    fn view_events_are_redacted_without_workspace_sync_capability() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel(2);
        let workspace_id = WorkspaceId::try_new("workspace").unwrap();
        let event = EventEnvelope {
            event_sequence: client_api::EventSequence::new(8),
            body: EventBody::WorkspaceViewChanged {
                workspace_id: workspace_id.clone(),
                revision: 3,
                view: Some(shared_view()),
            },
        };

        let redacting = TokioEventSink {
            sender: sender.clone(),
            can_read_git: true,
            can_read_view: false,
        };
        assert!(redacting.try_send(event.clone()));
        let received = receiver.try_recv().unwrap();
        assert_eq!(received.event_sequence.value(), 8);
        let EventBody::WorkspaceViewChanged {
            workspace_id: id,
            revision,
            view,
        } = received.body
        else {
            panic!("view event をそのまま配る")
        };
        assert_eq!(id, workspace_id);
        assert_eq!(revision, 3);
        assert!(view.is_none(), "権限のない Session には内容を伏せる");

        let passing = TokioEventSink {
            sender,
            can_read_git: true,
            can_read_view: true,
        };
        assert!(passing.try_send(event));
        let received = receiver.try_recv().unwrap();
        let EventBody::WorkspaceViewChanged { view, .. } = received.body else {
            panic!("view event をそのまま配る")
        };
        assert_eq!(view, Some(shared_view()));
    }

    #[test]
    fn view_state_content_reaches_only_sessions_with_workspace_sync() {
        let root = tempfile::tempdir().unwrap();
        let workspace_id = WorkspaceId::try_new("workspace-1").unwrap();
        let state = BackendState::new([WorkspaceRegistration::new(
            workspace_id.clone(),
            "Workspace",
            root.path().to_owned(),
            Arc::new(EmptyFiles),
        )])
        .unwrap();
        let handler = BackendRemoteHandler::new(BackendService::start(state).unwrap());
        let viewer = session_grant(
            "viewer",
            &workspace_id,
            &[
                client_api::capability::WORKSPACES_CONNECT,
                client_api::capability::WORKSPACE_SYNC,
                client_api::capability::WORKBENCH_WRITE,
            ],
        );
        // 共有を購読できない Session も接続でき、event の連番は同じものを受け取る。
        let blind = session_grant(
            "blind",
            &workspace_id,
            &[client_api::capability::WORKSPACES_CONNECT],
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        for (number, grant) in [(1_u64, viewer.clone()), (2, blind.clone())] {
            let connected = runtime.block_on(handler.handle(
                grant,
                request(
                    number,
                    RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
                        workspace_id: workspace_id.clone(),
                    }),
                ),
            ));
            assert!(matches!(connected.body, ResponseBody::WorkspaceConnect(_)));
        }
        let mut viewer_events = runtime
            .block_on(handler.subscribe(viewer.clone()))
            .unwrap()
            .expect("Session へ event を配る");
        let mut blind_events = runtime
            .block_on(handler.subscribe(blind.clone()))
            .unwrap()
            .expect("Session へ event を配る");

        let subscribed = runtime.block_on(handler.handle(
            viewer.clone(),
            request(
                3,
                RequestBody::WorkspaceViewSubscribe(WorkspaceViewSubscribeRequest {
                    workspace_id: workspace_id.clone(),
                }),
            ),
        ));
        assert!(matches!(
            subscribed.body,
            ResponseBody::WorkspaceViewSubscribe(_)
        ));

        let mut blind_sequence = None;
        for (number, revision) in [(4_u64, 1_u64), (5, 2)] {
            let updated = runtime.block_on(handler.handle(
                viewer.clone(),
                request(
                    number,
                    RequestBody::WorkspaceViewUpdate(WorkspaceViewUpdateRequest {
                        workspace_id: workspace_id.clone(),
                        view: shared_view(),
                    }),
                ),
            ));
            assert!(matches!(updated.body, ResponseBody::WorkspaceViewUpdate(_)));

            let viewer_event = next_view_event(&runtime, &mut viewer_events);
            let blind_event = next_view_event(&runtime, &mut blind_events);
            assert_eq!(blind_event.event_sequence, viewer_event.event_sequence);
            if let Some(previous) = blind_sequence {
                assert_eq!(blind_event.event_sequence.value(), previous + 1);
            }
            blind_sequence = Some(blind_event.event_sequence.value());

            let EventBody::WorkspaceViewChanged {
                revision: seen,
                view,
                ..
            } = viewer_event.body
            else {
                unreachable!("next_view_event は view event に限定している")
            };
            assert_eq!(seen, revision);
            assert_eq!(
                view.expect("workspace.sync を持つ Session は内容を受け取る")
                    .active_path,
                Some(WorkspaceRelativePath::parse("src/secret.rs").unwrap())
            );

            let EventBody::WorkspaceViewChanged {
                revision: seen,
                view,
                ..
            } = blind_event.body
            else {
                unreachable!("next_view_event は view event に限定している")
            };
            assert_eq!(seen, revision);
            assert!(
                view.is_none(),
                "workspace.sync を持たない Session に view の内容を配らない"
            );
        }
        handler.disconnect(&viewer);
        handler.disconnect(&blind);
    }

    fn session_grant(
        session: &str,
        workspace_id: &WorkspaceId,
        capabilities: &[&str],
    ) -> SessionGrant {
        SessionGrant::from_authenticated(
            SessionId::try_new(session).unwrap(),
            workspace_id.clone(),
            CapabilitySet::try_new(
                capabilities
                    .iter()
                    .map(|value| CapabilityId::try_new(*value).unwrap()),
            )
            .unwrap(),
        )
    }

    fn shared_view() -> WorkspaceView {
        WorkspaceView {
            open_documents: vec![WorkspaceDocumentView {
                document_id: None,
                path: Some(WorkspaceRelativePath::parse("src/secret.rs").unwrap()),
                cursor_char: 12,
                selection_start: 0,
                selection_end: 0,
                scroll_x: 0.0,
                scroll_y: 8.0,
            }],
            active_path: Some(WorkspaceRelativePath::parse("src/secret.rs").unwrap()),
            active_document_id: None,
        }
    }

    fn next_view_event(
        runtime: &tokio::runtime::Runtime,
        events: &mut RemoteEventReceiver,
    ) -> EventEnvelope {
        let event = runtime
            .block_on(async {
                tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
                    .await
                    .expect("event が届かない")
            })
            .expect("event receiver が閉じた");
        assert!(
            matches!(event.body, EventBody::WorkspaceViewChanged { .. }),
            "view event だけを配る"
        );
        event
    }

    fn request(number: u64, body: RequestBody) -> RequestEnvelope {
        RequestEnvelope {
            request_id: RequestId::try_new(format!("request-{number}")).unwrap(),
            body,
        }
    }
}
