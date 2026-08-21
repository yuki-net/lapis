use lapis_backend_state::{BackendService, BackendSession};
use lapis_client_api::{
    ErrorCode, INTERNAL, ProtocolError, RequestEnvelope, ResponseBody, ResponseEnvelope,
};

use crate::{RemoteRequestHandler, RemoteResponseFuture, SessionGrant};

#[derive(Clone)]
pub struct BackendRemoteHandler {
    backend: BackendService,
}

impl BackendRemoteHandler {
    pub fn new(backend: BackendService) -> Self {
        Self { backend }
    }
}

impl RemoteRequestHandler for BackendRemoteHandler {
    fn handle(&self, grant: SessionGrant, request: RequestEnvelope) -> RemoteResponseFuture<'_> {
        let backend = self.backend.clone();
        let request_id = request.request_id.clone();
        let session = BackendSession::new(grant.session_id().clone(), grant.workspace_id().clone());
        Box::pin(async move {
            match tokio::task::spawn_blocking(move || backend.dispatch(session, request)).await {
                Ok(Ok(response)) => response,
                Ok(Err(_)) | Err(_) => internal_error(request_id),
            }
        })
    }

    fn disconnect(&self, grant: &SessionGrant) {
        let _ = self.backend.disconnect(grant.session_id().clone());
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
        CapabilitySet, RequestBody, RequestId, ResponseBody, SessionId, SnapshotReason,
        SnapshotRequest, WorkspaceConnectRequest, WorkspaceId,
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
