use crate::BackendHost;
use lapis_backend_state::{BackendServiceError, BackendSession, BackendStateError};
use lapis_client_api::*;
use lapis_remote::{
    BackendRemoteHandler, RemoteRequestHandler, RemoteResponseFuture, RemoteSubscriptionFuture,
    SessionGrant,
};
use std::path::Path;

pub struct HostRemoteHandler {
    auth: Option<lapis_remote::SharedRemoteAuth>,
    host: BackendHost,
    inner: BackendRemoteHandler,
}
impl HostRemoteHandler {
    pub fn new(host: BackendHost) -> Self {
        let inner = BackendRemoteHandler::new(host.service());
        Self {
            host,
            inner,
            auth: None,
        }
    }
    pub fn with_auth(mut self, auth: lapis_remote::SharedRemoteAuth) -> Self {
        self.auth = Some(auth);
        self
    }
}
impl RemoteRequestHandler for HostRemoteHandler {
    fn handle(&self, grant: SessionGrant, request: RequestEnvelope) -> RemoteResponseFuture<'_> {
        if self.auth.is_none() && matches!(request.body, RequestBody::WorkspaceList(_)) {
            return self.inner.handle(grant, request);
        }
        if !matches!(
            request.body,
            RequestBody::HostRegister(_)
                | RequestBody::DocumentRelocate(_)
                | RequestBody::WorktreeOpen(_)
                | RequestBody::WorkspaceList(_)
        ) {
            return self.inner.handle(grant, request);
        }
        let host = self.host.clone();
        let auth = self.auth.clone();
        Box::pin(async move {
            let id = request.request_id.clone();
            let allowed = grant
                .require_capability(
                    &CapabilityId::try_new(request.body.required_capability())
                        .expect("built-in capability"),
                )
                .is_ok()
                && request
                    .body
                    .workspace_id()
                    .is_none_or(|workspace| grant.require_workspace(workspace).is_ok());
            if !allowed {
                return ResponseEnvelope {
                    request_id: id,
                    body: error(FORBIDDEN),
                };
            }
            let body = tokio::task::spawn_blocking(move || {
                match request.body {
                    RequestBody::WorkspaceList(_) => {
                        let Some(auth) = auth else {
                            return error(UNSUPPORTED);
                        };
                        let ids = match auth.lock() {
                            Ok(auth) => match auth.project_workspaces(&grant) {
                                Ok(ids) => ids,
                                Err(_) => return error(FORBIDDEN),
                            },
                            Err(_) => return error(INTERNAL),
                        };
                        let Ok(roots) = host.inner.workspaces.lock() else {
                            return error(INTERNAL);
                        };
                        ResponseBody::WorkspaceList(WorkspaceListResponse {
                            workspaces: ids
                                .into_iter()
                                .filter_map(|workspace_id| {
                                    roots.iter().find(|(_, id)| *id == &workspace_id).map(
                                        |(root, _)| WorkspaceSummary {
                                            workspace_id,
                                            name: root
                                                .file_name()
                                                .unwrap_or_default()
                                                .to_string_lossy()
                                                .into_owned(),
                                        },
                                    )
                                })
                                .collect(),
                        })
                    }
                    RequestBody::WorktreeOpen(input) => {
                        let Some(auth) = auth else {
                            return error(UNSUPPORTED);
                        };
                        let path = match host
                            .service()
                            .worktree_path(input.workspace_id, input.worktree_id.clone())
                        {
                            Ok(path) => path,
                            Err(_) => return error(FORBIDDEN),
                        };
                        let workspace_id = match host.register_workspace(&path) {
                            Ok(workspace) => workspace,
                            Err(problem) => return service_error(problem),
                        };
                        let Ok(mut auth) = auth.lock() else {
                            return error(INTERNAL);
                        };
                        if auth
                            .grant_worktree_workspace(&grant, workspace_id.clone())
                            .is_err()
                        {
                            return error(FORBIDDEN);
                        }
                        ResponseBody::WorktreeOpen(WorkspaceSummary {
                            workspace_id,
                            name: format!("Worktree {}", input.worktree_id.as_str()),
                        })
                    }
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
