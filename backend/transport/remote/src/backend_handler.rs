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
        let pending_subscriptions = self.pending_subscriptions.clone();
        Box::pin(async move {
            let receiver = opens_workspace.then(|| {
                let (sender, receiver) = tokio::sync::mpsc::channel(REMOTE_EVENT_QUEUE_CAPACITY);
                (
                    Arc::new(TokioEventSink { sender }) as Arc<dyn BackendEventSink>,
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
                Ok(Ok(response)) => {
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
}

impl BackendEventSink for TokioEventSink {
    fn try_send(&self, event: EventEnvelope) -> bool {
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
        CapabilitySet, DocumentCreateRequest, DocumentEncoding, EventBody, RequestBody, RequestId,
        ResponseBody, SessionId, SnapshotReason, SnapshotRequest, WorkspaceConnectRequest,
        WorkspaceId, WorkspaceRelativePath,
    };
    use lapis_document::{DocumentError, DocumentRepository, FileData, FileFingerprint};

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

    fn request(number: u64, body: RequestBody) -> RequestEnvelope {
        RequestEnvelope {
            request_id: RequestId::try_new(format!("request-{number}")).unwrap(),
            body,
        }
    }
}
