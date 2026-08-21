use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
    time::Duration,
};

use lapis_client_api::{
    CURRENT_PROTOCOL, ErrorCode, FORBIDDEN, INTERNAL, INVALID_REQUEST, PROTOCOL_ERROR,
    ProtocolError, ProtocolRange, RATE_LIMITED, RequestEnvelope, RequestId, ResponseBody,
    ResponseEnvelope, ServerHello, UNAUTHORIZED,
};

use crate::{
    AccessError, AuthError, CredentialHandle, PairingToken, RemoteAuth, RemoteRequestHandler,
    SessionGrant,
    wire::{
        AuthenticateRequest, ClientMessage, PairRequest, PairedResponse, ServerMessage,
        protocol_error,
    },
};

pub type SharedRemoteAuth = Arc<Mutex<RemoteAuth>>;

pub(crate) struct SessionReply {
    pub(crate) message: ServerMessage,
    pub(crate) close: bool,
}

pub(crate) struct ConnectionSession {
    grant: Option<SessionGrant>,
    request_ids: HashSet<RequestId>,
}

const MAX_REQUESTS_PER_SESSION: usize = 65_536;

impl ConnectionSession {
    pub(crate) fn new() -> Self {
        Self {
            grant: None,
            request_ids: HashSet::new(),
        }
    }

    pub(crate) fn is_authenticated(&self) -> bool {
        self.grant.is_some()
    }

    pub(crate) fn grant(&self) -> Option<&SessionGrant> {
        self.grant.as_ref()
    }

    pub(crate) async fn receive(
        &mut self,
        message: ClientMessage,
        auth: &SharedRemoteAuth,
        handler: &dyn RemoteRequestHandler,
        request_timeout: Duration,
    ) -> SessionReply {
        match (self.grant.is_some(), message) {
            (false, ClientMessage::Pair(request)) => self.pair(request, auth),
            (false, ClientMessage::Authenticate(request)) => self.authenticate(request, auth),
            (false, ClientMessage::Request(_)) => SessionReply {
                message: protocol_error(UNAUTHORIZED),
                close: true,
            },
            (true, ClientMessage::Request(request)) => {
                self.handle_request(request, auth, handler, request_timeout)
                    .await
            }
            (true, ClientMessage::Pair(_) | ClientMessage::Authenticate(_)) => SessionReply {
                message: protocol_error(PROTOCOL_ERROR),
                close: true,
            },
        }
    }

    pub(crate) fn disconnect(
        &mut self,
        auth: &SharedRemoteAuth,
        handler: &dyn RemoteRequestHandler,
    ) {
        let Some(grant) = self.grant.take() else {
            return;
        };
        handler.disconnect(&grant);
        if let Ok(mut auth) = auth.lock() {
            auth.disconnect(grant.session_id());
        }
    }

    fn pair(&mut self, mut request: PairRequest, auth: &SharedRemoteAuth) -> SessionReply {
        let protocol = match negotiate(&request.hello) {
            Ok(protocol) => protocol,
            Err(_) => return protocol_rejected(),
        };
        let mut token_bytes = match request.take_token() {
            Ok(token) => token,
            Err(_) => return authentication_rejected(),
        };
        let token = PairingToken::from_secret_bytes(&mut token_bytes);
        let result = auth
            .lock()
            .map_err(|_| AuthError::CredentialUnavailable)
            .and_then(|mut auth| {
                auth.complete_pairing_and_authenticate(
                    &token,
                    &request.hello.client_id,
                    &request.hello.requested_capabilities,
                )
            });
        let (credential, grant) = match result {
            Ok(result) => result,
            Err(_) => return authentication_rejected(),
        };
        let hello = server_hello(protocol, &grant);
        let response = PairedResponse::new(hello, credential.id(), credential.secret_bytes());
        self.grant = Some(grant);
        SessionReply {
            message: ServerMessage::Paired(response),
            close: false,
        }
    }

    fn authenticate(
        &mut self,
        mut request: AuthenticateRequest,
        auth: &SharedRemoteAuth,
    ) -> SessionReply {
        let protocol = match negotiate(&request.hello) {
            Ok(protocol) => protocol,
            Err(_) => return protocol_rejected(),
        };
        let credential_id = match request.credential_id() {
            Ok(id) => id,
            Err(_) => return authentication_rejected(),
        };
        let mut secret = match request.take_secret() {
            Ok(secret) => secret,
            Err(_) => return authentication_rejected(),
        };
        let credential = CredentialHandle::from_parts(credential_id, &mut secret);
        let grant = match auth
            .lock()
            .map_err(|_| AuthError::CredentialUnavailable)
            .and_then(|mut auth| {
                auth.authenticate(&credential, &request.hello.client_id, &request.workspace_id)
            }) {
            Ok(grant) => grant,
            Err(_) => return authentication_rejected(),
        };
        let hello = server_hello(protocol, &grant);
        self.grant = Some(grant);
        SessionReply {
            message: ServerMessage::Authenticated(hello),
            close: false,
        }
    }

    async fn handle_request(
        &mut self,
        request: RequestEnvelope,
        auth: &SharedRemoteAuth,
        handler: &dyn RemoteRequestHandler,
        request_timeout: Duration,
    ) -> SessionReply {
        if self.request_ids.len() >= MAX_REQUESTS_PER_SESSION {
            return SessionReply {
                message: ServerMessage::Response(error_response(&request, RATE_LIMITED)),
                close: true,
            };
        }
        if !self.request_ids.insert(request.request_id.clone()) {
            return SessionReply {
                message: ServerMessage::Response(error_response(&request, INVALID_REQUEST)),
                close: false,
            };
        }
        let grant = self
            .grant
            .as_ref()
            .expect("authenticated branch must contain a grant");
        let access = auth
            .lock()
            .map_err(|_| AccessError::Authentication(AuthError::CredentialUnavailable))
            .and_then(|auth| auth.authorize_request(grant, &request.body));
        if let Err(error) = access {
            return access_rejected(&request, error);
        }

        let response = match tokio::time::timeout(
            request_timeout,
            handler.handle(grant.clone(), request.clone()),
        )
        .await
        {
            Ok(response) => response,
            Err(_) => {
                let mut response = error_response(&request, INTERNAL);
                if let ResponseBody::Error(error) = &mut response.body {
                    error.retryable = true;
                }
                return SessionReply {
                    message: ServerMessage::Response(response),
                    close: false,
                };
            }
        };
        if !response.matches_request(&request) {
            return SessionReply {
                message: ServerMessage::Response(error_response(&request, INTERNAL)),
                close: false,
            };
        }
        SessionReply {
            message: ServerMessage::Response(response),
            close: false,
        }
    }
}

fn negotiate(
    hello: &lapis_client_api::ClientHello,
) -> Result<lapis_client_api::ProtocolVersion, lapis_client_api::VersionMismatch> {
    ProtocolRange::exact(CURRENT_PROTOCOL).negotiate(hello.protocol)
}

fn protocol_rejected() -> SessionReply {
    SessionReply {
        message: protocol_error(PROTOCOL_ERROR),
        close: true,
    }
}

fn server_hello(protocol: lapis_client_api::ProtocolVersion, grant: &SessionGrant) -> ServerHello {
    ServerHello {
        protocol,
        session_id: grant.session_id().clone(),
        granted_capabilities: grant.capabilities().clone(),
    }
}

fn authentication_rejected() -> SessionReply {
    SessionReply {
        message: protocol_error(UNAUTHORIZED),
        close: true,
    }
}

fn access_rejected(request: &RequestEnvelope, error: AccessError) -> SessionReply {
    let (code, close) = match error {
        AccessError::Authentication(_) => (UNAUTHORIZED, true),
        AccessError::Authorization(_) => (FORBIDDEN, false),
    };
    SessionReply {
        message: ServerMessage::Response(error_response(request, code)),
        close,
    }
}

fn error_response(request: &RequestEnvelope, code: &str) -> ResponseEnvelope {
    ResponseEnvelope {
        request_id: request.request_id.clone(),
        body: ResponseBody::Error(ProtocolError::new(
            ErrorCode::try_new(code).expect("built-in protocol error code must be valid"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use lapis_client_api::{
        CapabilityId, CapabilitySet, ClientHello, ClientId, ClientKind, ProtocolRange, RequestBody,
        RequestId, ResponseBody, WorkspaceId, WorkspaceListRequest, WorkspaceListResponse,
        capability,
    };

    use crate::{AuthConfig, AuthPolicy, CredentialLifetime, PairingLifetime};

    struct ListHandler;

    impl RemoteRequestHandler for ListHandler {
        fn handle(
            &self,
            _session: SessionGrant,
            request: RequestEnvelope,
        ) -> crate::RemoteResponseFuture<'_> {
            Box::pin(async move {
                ResponseEnvelope {
                    request_id: request.request_id,
                    body: ResponseBody::WorkspaceList(WorkspaceListResponse {
                        workspaces: Vec::new(),
                    }),
                }
            })
        }
    }

    struct PendingHandler;

    impl RemoteRequestHandler for PendingHandler {
        fn handle(
            &self,
            _session: SessionGrant,
            _request: RequestEnvelope,
        ) -> crate::RemoteResponseFuture<'_> {
            Box::pin(std::future::pending())
        }
    }

    fn auth() -> SharedRemoteAuth {
        let capability = CapabilityId::try_new(capability::WORKSPACES).unwrap();
        let config = AuthConfig::new(
            PairingLifetime::new(Duration::from_secs(30).as_secs()).unwrap(),
            CredentialLifetime::new(Duration::from_secs(300).as_secs()).unwrap(),
        );
        let mut auth = RemoteAuth::system(
            config,
            AuthPolicy::new(CapabilitySet::try_new([capability]).unwrap()),
        );
        auth.enable();
        Arc::new(Mutex::new(auth))
    }

    fn hello() -> ClientHello {
        ClientHello {
            protocol: ProtocolRange::exact(CURRENT_PROTOCOL),
            client_id: ClientId::try_new("mobile-1").unwrap(),
            client_name: "Mobile".to_owned(),
            client_kind: ClientKind::Android,
            requested_capabilities: CapabilitySet::try_new([CapabilityId::try_new(
                capability::WORKSPACES,
            )
            .unwrap()])
            .unwrap(),
        }
    }

    #[test]
    fn request_before_authentication_is_rejected_and_closed() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let mut session = ConnectionSession::new();
            let reply = session
                .receive(
                    ClientMessage::Request(list_request()),
                    &auth(),
                    &ListHandler,
                    Duration::from_secs(1),
                )
                .await;

            assert!(reply.close);
            assert!(matches!(reply.message, ServerMessage::Error(_)));
        });
    }

    #[test]
    fn pairing_authenticates_then_dispatches_authorized_requests() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let auth = auth();
            let workspace = WorkspaceId::try_new("workspace-1").unwrap();
            let token = auth
                .lock()
                .unwrap()
                .begin_pairing(workspace.clone())
                .unwrap();
            let mut session = ConnectionSession::new();
            let paired = session
                .receive(
                    ClientMessage::Pair(PairRequest::new(hello(), token.secret_bytes())),
                    &auth,
                    &ListHandler,
                    Duration::from_secs(1),
                )
                .await;

            assert!(!paired.close);
            assert!(session.is_authenticated());
            let paired_json = serde_json::to_string(&paired.message).unwrap();
            assert!(matches!(paired.message, ServerMessage::Paired(_)));

            let response = session
                .receive(
                    ClientMessage::Request(list_request()),
                    &auth,
                    &ListHandler,
                    Duration::from_secs(1),
                )
                .await;
            assert!(!response.close);
            assert!(matches!(
                response.message,
                ServerMessage::Response(ResponseEnvelope {
                    body: ResponseBody::WorkspaceList(_),
                    ..
                })
            ));
            let duplicate = session
                .receive(
                    ClientMessage::Request(list_request()),
                    &auth,
                    &ListHandler,
                    Duration::from_secs(1),
                )
                .await;
            assert!(matches!(
                duplicate.message,
                ServerMessage::Response(ResponseEnvelope {
                    body: ResponseBody::Error(ref error),
                    ..
                }) if error.code.as_str() == INVALID_REQUEST
            ));
            session.disconnect(&auth, &ListHandler);

            let mut paired = match serde_json::from_str::<ServerMessage>(&paired_json).unwrap() {
                ServerMessage::Paired(response) => response,
                _ => panic!("expected paired response"),
            };
            let credential_id = paired.credential_id().unwrap();
            let secret = paired.take_secret().unwrap();
            let mut reconnected = ConnectionSession::new();
            let authenticated = reconnected
                .receive(
                    ClientMessage::Authenticate(AuthenticateRequest::new(
                        hello(),
                        workspace,
                        &credential_id,
                        &secret,
                    )),
                    &auth,
                    &ListHandler,
                    Duration::from_secs(1),
                )
                .await;
            assert!(!authenticated.close);
            assert!(matches!(
                authenticated.message,
                ServerMessage::Authenticated(_)
            ));
            reconnected.disconnect(&auth, &ListHandler);
        });
    }

    #[test]
    fn backend_request_timeout_returns_retryable_error() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let auth = auth();
            let workspace = WorkspaceId::try_new("workspace-1").unwrap();
            let token = auth.lock().unwrap().begin_pairing(workspace).unwrap();
            let mut session = ConnectionSession::new();
            session
                .receive(
                    ClientMessage::Pair(PairRequest::new(hello(), token.secret_bytes())),
                    &auth,
                    &ListHandler,
                    Duration::from_secs(1),
                )
                .await;

            let reply = session
                .receive(
                    ClientMessage::Request(list_request()),
                    &auth,
                    &PendingHandler,
                    Duration::from_millis(1),
                )
                .await;
            let ServerMessage::Response(ResponseEnvelope {
                body: ResponseBody::Error(error),
                ..
            }) = reply.message
            else {
                panic!("expected timeout error response");
            };
            assert_eq!(error.code.as_str(), INTERNAL);
            assert!(error.retryable);
            assert!(!reply.close);
            session.disconnect(&auth, &PendingHandler);
        });
    }

    fn list_request() -> RequestEnvelope {
        RequestEnvelope {
            request_id: RequestId::try_new("request-1").unwrap(),
            body: RequestBody::WorkspaceList(WorkspaceListRequest::default()),
        }
    }
}
