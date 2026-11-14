use crate::BackendHost;
use backend_state::{BackendServiceError, BackendSession, BackendStateError};
use client_api::*;
use remote::{
    BackendRemoteHandler, RemoteRequestHandler, RemoteResponseFuture, RemoteSubscriptionFuture,
    SessionGrant, SharedRemoteAuth,
};
use std::{
    path::Path,
    sync::{Arc, OnceLock},
};

pub struct HostRemoteHandler {
    host: BackendHost,
    inner: BackendRemoteHandler,
    auth: Option<SharedRemoteAuth>,
    web: Option<Arc<OnceLock<Arc<crate::web_publisher::WebPublisher>>>>,
}
impl HostRemoteHandler {
    pub fn new(host: BackendHost) -> Self {
        let inner = BackendRemoteHandler::new(host.service());
        Self {
            host,
            inner,
            auth: None,
            web: None,
        }
    }

    /// managed Host が所有する認証サービスを管理操作へ公開する。
    pub fn with_auth(mut self, auth: SharedRemoteAuth) -> Self {
        self.auth = Some(auth);
        self
    }

    pub(crate) fn with_web(
        mut self,
        web: Arc<OnceLock<Arc<crate::web_publisher::WebPublisher>>>,
    ) -> Self {
        self.web = Some(web);
        self
    }
}
impl RemoteRequestHandler for HostRemoteHandler {
    fn handle(&self, grant: SessionGrant, request: RequestEnvelope) -> RemoteResponseFuture<'_> {
        if !matches!(
            request.body,
            RequestBody::HostRegister(_)
                | RequestBody::HostWorktree(_)
                | RequestBody::HostInvite(_)
                | RequestBody::HostDeviceList(_)
                | RequestBody::HostDeviceRevoke(_)
                | RequestBody::HostWebStatus(_)
                | RequestBody::HostWebStart(_)
                | RequestBody::HostWebStop(_)
                | RequestBody::HostExtensions(_)
                | RequestBody::DocumentRelocate(_)
        ) {
            return self.inner.handle(grant, request);
        }
        let host = self.host.clone();
        let auth = self.auth.clone();
        let web = self.web.as_ref().and_then(|holder| holder.get()).cloned();
        Box::pin(async move {
            let id = request.request_id.clone();
            // 拡張の宣言は通常の Client が表示に使う。`host.manage` は要求せず、
            // 読み取り専用の capability だけで許可する。
            let allowed = if matches!(request.body, RequestBody::HostExtensions(_)) {
                grant
                    .capabilities()
                    .iter()
                    .any(|capability| capability.as_str() == capability::EXTENSIONS_READ)
            } else {
                let owner = grant
                    .capabilities()
                    .iter()
                    .any(|capability| capability.as_str() == capability::HOST_MANAGE);
                owner
                    || (matches!(request.body, RequestBody::HostRegister(_))
                        && grant.capabilities().iter().any(|capability| {
                            capability.as_str() == capability::WORKSPACES_REGISTER
                        }))
            };
            if !allowed {
                return ResponseEnvelope {
                    request_id: id,
                    body: error(FORBIDDEN),
                };
            }
            let body = tokio::task::spawn_blocking(move || {
                match request.body {
                    RequestBody::HostWebStatus(_) => {
                        let Some(web) = web else {
                            return error(UNSUPPORTED);
                        };
                        match web.status() {
                            Ok(status) => ResponseBody::HostWebStatus(status),
                            Err(message) => web_error(message),
                        }
                    }
                    RequestBody::HostWebStart(request) => {
                        let Some(web) = web else {
                            return error(UNSUPPORTED);
                        };
                        let Ok(ip) = request.address.parse() else {
                            return error(INVALID_REQUEST);
                        };
                        match web.start(std::net::SocketAddr::new(ip, request.port)) {
                            Ok(status) => ResponseBody::HostWebStart(status),
                            Err(message) => web_error(message),
                        }
                    }
                    RequestBody::HostWebStop(_) => {
                        let Some(web) = web else {
                            return error(UNSUPPORTED);
                        };
                        match web.stop() {
                            Ok(status) => ResponseBody::HostWebStop(status),
                            Err(message) => web_error(message),
                        }
                    }
                    RequestBody::HostDeviceList(_) => {
                        let Some(auth) = auth else {
                            return error(UNSUPPORTED);
                        };
                        let Ok(auth) = auth.lock() else {
                            return error(INTERNAL);
                        };
                        let current_client = auth.session_client_id(grant.session_id());
                        let devices = auth
                            .devices()
                            .into_iter()
                            .map(|device| {
                                let is_current = current_client == Some(device.client_id.as_str());
                                HostDevice {
                                    id: device.id,
                                    client_id: device.client_id,
                                    name: device.name,
                                    kind: device.kind,
                                    status: match device.status {
                                        backend_state::DeviceStatus::Pending => "pending",
                                        backend_state::DeviceStatus::Approved => "approved",
                                        backend_state::DeviceStatus::Revoked => "revoked",
                                    }
                                    .to_owned(),
                                    last_seen_at: device.last_seen_at,
                                    is_current,
                                }
                            })
                            .collect();
                        ResponseBody::HostDeviceList(HostDeviceListResponse { devices })
                    }
                    RequestBody::HostDeviceRevoke(request) => {
                        let Some(auth) = auth else {
                            return error(UNSUPPORTED);
                        };
                        let Ok(mut auth) = auth.lock() else {
                            return error(INTERNAL);
                        };
                        // 自身の端末を失効させると Host 管理 credential を失い、
                        // 再起動まで操作できなくなるため Host 側でも拒否する。
                        let current = auth
                            .session_client_id(grant.session_id())
                            .map(str::to_owned);
                        if let Some(current) = current
                            && auth.devices().iter().any(|device| {
                                device.id == request.device_id && device.client_id == current
                            })
                        {
                            return error(INVALID_REQUEST);
                        }
                        match auth.revoke_device_and_persist(&request.device_id) {
                            Ok(revoked) => {
                                ResponseBody::HostDeviceRevoke(HostDeviceRevokeResponse { revoked })
                            }
                            Err(_) => error(INTERNAL),
                        }
                    }
                    RequestBody::HostInvite(_) => {
                        let Some(auth) = auth else {
                            return error(UNSUPPORTED);
                        };
                        let Ok(mut auth) = auth.lock() else {
                            return error(INTERNAL);
                        };
                        match auth.begin_user_session_pairing(grant.workspace_id().clone()) {
                            Ok(token) => {
                                use std::fmt::Write;
                                let mut encoded = String::with_capacity(64);
                                for byte in token.secret_bytes() {
                                    write!(encoded, "{byte:02x}").expect("string formatting");
                                }
                                ResponseBody::HostInvite(HostInviteResponse { token: encoded })
                            }
                            Err(_) => error(INTERNAL),
                        }
                    }
                    RequestBody::HostExtensions(_) => match host.extensions() {
                        Ok(list) => ResponseBody::HostExtensions(list),
                        Err(message) => web_error(message),
                    },
                    RequestBody::HostWorktree(worktree) => {
                        match host
                            .service()
                            .worktree_path(grant.workspace_id().clone(), worktree)
                        {
                            Ok(path) => match path.to_str() {
                                Some(path) => ResponseBody::HostWorktree(path.to_owned()),
                                None => error(INVALID_PATH),
                            },
                            Err(problem) => service_error(problem),
                        }
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
    fn default_workspace(&self) -> Option<WorkspaceId> {
        self.host.default_workspace()
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

fn web_error(message: String) -> ResponseBody {
    ResponseBody::Error(
        ProtocolError::new(ErrorCode::try_new(INTERNAL).expect("built-in error"))
            .with_detail(message),
    )
}
fn service_error(problem: BackendServiceError) -> ResponseBody {
    match problem {
        BackendServiceError::State(BackendStateError::RevisionConflict(conflict)) => {
            ResponseBody::Error(ProtocolError::revision_conflict(conflict))
        }
        BackendServiceError::State(BackendStateError::Document(problem))
            if problem.kind() == document::DocumentErrorKind::Conflict =>
        {
            error("document_conflict")
        }
        _ => error(INTERNAL),
    }
}
