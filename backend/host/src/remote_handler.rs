use crate::BackendHost;
use lapis_backend_state::{BackendServiceError, BackendSession, BackendStateError};
use lapis_client_api::*;
use lapis_remote::{
    BackendRemoteHandler, RemoteRequestHandler, RemoteResponseFuture, RemoteSubscriptionFuture,
    SessionGrant,
};
use std::path::Path;

pub struct HostRemoteHandler {
    host: BackendHost,
    inner: BackendRemoteHandler,
}
impl HostRemoteHandler {
    pub fn new(host: BackendHost) -> Self {
        let inner = BackendRemoteHandler::new(host.service());
        Self { host, inner }
    }
}
impl RemoteRequestHandler for HostRemoteHandler {
    fn handle(&self, grant: SessionGrant, request: RequestEnvelope) -> RemoteResponseFuture<'_> {
        if !matches!(
            request.body,
            RequestBody::HostRegister(_) | RequestBody::DocumentRelocate(_)
        ) {
            return self.inner.handle(grant, request);
        }
        let host = self.host.clone();
        Box::pin(async move {
            let id = request.request_id.clone();
            let allowed = grant
                .capabilities()
                .iter()
                .any(|capability| capability.as_str() == capability::HOST_MANAGE);
            if !allowed {
                return ResponseEnvelope {
                    request_id: id,
                    body: error(FORBIDDEN),
                };
            }
            let body = tokio::task::spawn_blocking(move || {
                match request.body {
                    RequestBody::HostRegister(input) => {
                        let root = Path::new(&input.root);
                        if !root.is_absolute() {
                            return error(INVALID_PATH);
                        }
                        match host.register_workspace(root) {
                            Ok(workspace_id) => ResponseBody::HostRegister(WorkspaceSummary {
                                workspace_id,
                                name: root
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .into_owned(),
                            }),
                            Err(problem) => service_error(problem),
                        }
                    }
                    RequestBody::DocumentRelocate(input) => {
                        // 管理権限に加え、移動元sessionがこのDocumentへattachしていることを確認。
                        let inspected = host.service().dispatch(
                            BackendSession::new(
                                grant.session_id().clone(),
                                grant.workspace_id().clone(),
                            ),
                            RequestEnvelope {
                                request_id: request.request_id,
                                body: RequestBody::DocumentInspect(DocumentInspectRequest {
                                    document_id: input.document_id.clone(),
                                }),
                            },
                        );
                        match inspected {
                            Ok(response)
                                if matches!(response.body, ResponseBody::DocumentInspect(_)) => {}
                            Ok(response) => return response.body,
                            Err(problem) => return service_error(problem),
                        }
                        match host.service().relocate_document(
                            grant.workspace_id().clone(),
                            input.destination_workspace,
                            input.document_id.clone(),
                            input.base_revision,
                            input.path,
                        ) {
                            Ok(()) => ResponseBody::DocumentRelocate(DocumentCloseResponse {
                                document_id: input.document_id,
                            }),
                            Err(problem) => service_error(problem),
                        }
                    }
                    _ => error(INTERNAL),
                }
            })
            .await
            .unwrap_or_else(|_| error(INTERNAL));
            ResponseEnvelope {
                request_id: id,
                body,
            }
        })
    }
    fn subscribe(&self, grant: SessionGrant) -> RemoteSubscriptionFuture<'_> {
        self.inner.subscribe(grant)
    }
    fn disconnect(&self, grant: &SessionGrant) {
        self.inner.disconnect(grant);
    }
}
fn error(code: &str) -> ResponseBody {
    ResponseBody::Error(ProtocolError::new(
        ErrorCode::try_new(code).expect("built-in error"),
    ))
}
fn service_error(problem: BackendServiceError) -> ResponseBody {
    match problem {
        BackendServiceError::State(BackendStateError::RevisionConflict(conflict)) => {
            ResponseBody::Error(ProtocolError::revision_conflict(conflict))
        }
        BackendServiceError::State(BackendStateError::Document(problem))
            if problem.kind() == lapis_document::DocumentErrorKind::Conflict =>
        {
            error("document_conflict")
        }
        _ => error(INTERNAL),
    }
}
