use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use crate::{BackendEventReceiver, BackendService, BackendServiceError, BackendSession};
use lapis_client_api::{
    RequestBody, RequestEnvelope, RequestId, ResponseBody, SessionId, WorkspaceConnectRequest,
    WorkspaceId,
};

#[derive(Clone)]
pub struct EmbeddedClient {
    inner: Arc<ClientInner>,
}

struct ClientInner {
    service: BackendService,
    session: BackendSession,
    next_request: AtomicU64,
}

impl EmbeddedClient {
    pub fn connect(
        service: BackendService,
        session_id: SessionId,
        workspace_id: WorkspaceId,
    ) -> Result<Self, BackendServiceError> {
        let client = Self {
            inner: Arc::new(ClientInner {
                service,
                session: BackendSession::new(session_id, workspace_id.clone()),
                next_request: AtomicU64::new(1),
            }),
        };
        match client.request(RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
            workspace_id,
        }))? {
            ResponseBody::WorkspaceConnect(_) => Ok(client),
            ResponseBody::Error(error) => Err(BackendServiceError::Rejected(error)),
            _ => Err(BackendServiceError::Unavailable),
        }
    }

    pub fn workspace_id(&self) -> &WorkspaceId {
        &self.inner.session.workspace_id
    }

    pub fn request(&self, body: RequestBody) -> Result<ResponseBody, BackendServiceError> {
        let sequence = self.inner.next_request.fetch_add(1, Ordering::Relaxed);
        let request = RequestEnvelope {
            request_id: RequestId::try_new(format!("embedded-{sequence}"))
                .expect("bounded request id"),
            body,
        };
        Ok(self
            .inner
            .service
            .dispatch(self.inner.session.clone(), request)?
            .body)
    }

    pub fn subscribe(&self) -> Result<BackendEventReceiver, BackendServiceError> {
        self.inner.service.subscribe(self.inner.session.clone())
    }
}

impl Drop for ClientInner {
    fn drop(&mut self) {
        let _ = self.service.disconnect(self.session.session_id.clone());
    }
}
