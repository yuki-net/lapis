use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use lapis_backend_state::{BackendEventSink, BackendService, BackendSession};
use lapis_client_api::{
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
        let session = BackendSession::new(grant.session_id().clone(), grant.workspace_id().clone());
        let opens_workspace = matches!(&request.body, RequestBody::WorkspaceConnect(_));
        let can_read_git = grant.capabilities().contains(
            &lapis_client_api::CapabilityId::try_new("workspace.git.read")
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
}

impl BackendEventSink for TokioEventSink {
    fn try_send(&self, mut event: EventEnvelope) -> bool {
        if !self.can_read_git
            && let lapis_client_api::EventBody::GitStatusChanged { workspace_id, .. }
            | lapis_client_api::EventBody::GitRepositoriesChanged { workspace_id }
            | lapis_client_api::EventBody::GitWorktreesChanged { workspace_id } = &event.body
        {
            event.body = lapis_client_api::EventBody::WorkspaceChanged {
                workspace_id: workspace_id.clone(),
            };
        }
        self.sender.try_send(event).is_ok()
    }
}

fn internal_error(request_id: lapis_client_api::RequestId) -> ResponseEnvelope {
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

    use lapis_backend_state::{
        BackendState, WorkspaceEntry, WorkspaceFileBackend, WorkspaceRegistration,
    };
    use lapis_client_api::{
        CapabilityId, WorkspaceOperation as Operation, WorkspaceOperationRequest,
        WorkspaceOperationResult as Outcome,
    };
    use lapis_client_api::{
        CapabilitySet, DocumentCreateRequest, DocumentEncoding, EventBody, RequestBody, RequestId,
        ResponseBody, SessionId, SnapshotReason, SnapshotRequest, WorkspaceConnectRequest,
        WorkspaceId, WorkspaceRelativePath,
    };
    use lapis_document::{DocumentError, DocumentRepository, FileData, FileFingerprint};
    use lapis_editor_core::{ConversationId, ExecutionId, TaskId};
    use lapis_task::{
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
        fn create_directory(
            &self,
            _: &std::path::Path,
        ) -> Result<(), lapis_document::DocumentError> {
            Err(lapis_document::DocumentError::io("Unsupported"))
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
        let embedded = lapis_backend_state::EmbeddedClient::connect(
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
        };
        let workspace_id = WorkspaceId::try_new("workspace").unwrap();
        let event = EventEnvelope {
            event_sequence: lapis_client_api::EventSequence::new(7),
            body: EventBody::GitStatusChanged {
                workspace_id: workspace_id.clone(),
                repository: lapis_git::GitRepositoryStatus {
                    repository_id: "path:private".into(),
                    status: lapis_git::GitSnapshot {
                        workspace_id: lapis_editor_core::WorkspaceId::new("workspace"),
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

    fn request(number: u64, body: RequestBody) -> RequestEnvelope {
        RequestEnvelope {
            request_id: RequestId::try_new(format!("request-{number}")).unwrap(),
            body,
        }
    }
}
