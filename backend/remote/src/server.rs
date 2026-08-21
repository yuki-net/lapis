use std::{
    error::Error,
    fmt,
    net::SocketAddr,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant as StdInstant},
};

use axum::{
    Router,
    extract::{
        ConnectInfo, State,
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade, close_code},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::any,
};
use axum_server::{Handle, tls_rustls::RustlsConfig};
use lapis_client_api::{
    ErrorCode, EventEnvelope, INTERNAL, PROTOCOL_ERROR, ProtocolError, ResponseBody,
    ResponseEnvelope, UNAUTHORIZED,
};
use tokio::time::{Instant as TokioInstant, timeout, timeout_at};

use crate::{
    RemoteEventReceiver, RemoteLimits, RemoteRequestHandler, Tls13ServerConfig,
    rate_limit::AuthenticationAttemptLimiter,
    session::{ConnectionSession, SharedRemoteAuth},
    wire::{
        ClientMessage, REMOTE_WEBSOCKET_PATH, REMOTE_WEBSOCKET_PROTOCOL, ServerMessage,
        protocol_error,
    },
};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

pub struct RemoteServerConfig {
    pub bind_address: SocketAddr,
    pub tls: Tls13ServerConfig,
    pub limits: RemoteLimits,
}

impl RemoteServerConfig {
    pub fn new(bind_address: SocketAddr, tls: Tls13ServerConfig, limits: RemoteLimits) -> Self {
        Self {
            bind_address,
            tls,
            limits,
        }
    }
}

pub struct RemoteServer {
    listening_address: SocketAddr,
    handle: Handle<SocketAddr>,
    thread: Option<thread::JoinHandle<Result<(), String>>>,
}

impl RemoteServer {
    pub fn start(
        config: RemoteServerConfig,
        auth: SharedRemoteAuth,
        handler: Arc<dyn RemoteRequestHandler>,
    ) -> Result<Self, RemoteServerError> {
        if !auth
            .lock()
            .map_err(|_| RemoteServerError::AuthenticationStateUnavailable)?
            .is_enabled()
        {
            return Err(RemoteServerError::RemoteDisabled);
        }

        let handle = Handle::new();
        let thread_handle = handle.clone();
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("lapis-remote".to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .thread_name("lapis-remote-worker")
                    .build()
                    .map_err(|error| error.to_string())?;
                runtime.block_on(run_server(config, auth, handler, thread_handle, started_tx))
            })
            .map_err(RemoteServerError::ThreadSpawn)?;

        match started_rx.recv_timeout(STARTUP_TIMEOUT) {
            Ok(Ok(listening_address)) => Ok(Self {
                listening_address,
                handle,
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                handle.shutdown();
                let _ = thread.join();
                Err(RemoteServerError::Startup(error))
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                handle.shutdown();
                let _ = thread.join();
                Err(RemoteServerError::StartupTimeout)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                handle.shutdown();
                let result = thread.join();
                let detail = match result {
                    Ok(Ok(())) => "remote thread ended before listening".to_owned(),
                    Ok(Err(error)) => error,
                    Err(_) => "remote thread panicked before listening".to_owned(),
                };
                Err(RemoteServerError::Startup(detail))
            }
        }
    }

    pub fn listening_address(&self) -> SocketAddr {
        self.listening_address
    }

    pub fn shutdown(mut self) -> Result<(), RemoteServerError> {
        self.handle.graceful_shutdown(Some(SHUTDOWN_TIMEOUT));
        self.join_thread()
    }

    fn join_thread(&mut self) -> Result<(), RemoteServerError> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        match thread.join() {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(RemoteServerError::Runtime(error)),
            Err(_) => Err(RemoteServerError::ThreadPanicked),
        }
    }
}

impl Drop for RemoteServer {
    fn drop(&mut self) {
        if self.thread.is_some() {
            self.handle.shutdown();
            let _ = self.join_thread();
        }
    }
}

#[derive(Debug)]
pub enum RemoteServerError {
    RemoteDisabled,
    AuthenticationStateUnavailable,
    ThreadSpawn(std::io::Error),
    StartupTimeout,
    Startup(String),
    Runtime(String),
    ThreadPanicked,
}

impl fmt::Display for RemoteServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RemoteDisabled => formatter.write_str("remote access is disabled"),
            Self::AuthenticationStateUnavailable => {
                formatter.write_str("remote authentication state is unavailable")
            }
            Self::ThreadSpawn(error) => write!(formatter, "failed to start remote thread: {error}"),
            Self::StartupTimeout => formatter.write_str("remote server startup timed out"),
            Self::Startup(error) => write!(formatter, "remote server failed to start: {error}"),
            Self::Runtime(error) => write!(formatter, "remote server failed: {error}"),
            Self::ThreadPanicked => formatter.write_str("remote server thread panicked"),
        }
    }
}

impl Error for RemoteServerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ThreadSpawn(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Clone)]
struct EndpointState {
    auth: SharedRemoteAuth,
    handler: Arc<dyn RemoteRequestHandler>,
    limits: RemoteLimits,
    authentication_attempts: AuthenticationAttemptLimiter,
}

async fn run_server(
    config: RemoteServerConfig,
    auth: SharedRemoteAuth,
    handler: Arc<dyn RemoteRequestHandler>,
    handle: Handle<SocketAddr>,
    started: mpsc::SyncSender<Result<SocketAddr, String>>,
) -> Result<(), String> {
    let state = EndpointState {
        auth,
        handler,
        limits: config.limits,
        authentication_attempts: AuthenticationAttemptLimiter::new(
            config.limits.authentication_rate_limit(),
        ),
    };
    let app = Router::new()
        .route(REMOTE_WEBSOCKET_PATH, any(upgrade_websocket))
        .with_state(state);
    let rustls = RustlsConfig::from_config(config.tls.into_inner());
    let listening_handle = handle.clone();
    let mut server = Box::pin(
        axum_server::bind_rustls(config.bind_address, rustls)
            .handle(handle)
            .serve(app.into_make_service_with_connect_info::<SocketAddr>()),
    );

    tokio::select! {
        result = &mut server => {
            let result = result.map_err(|error| error.to_string());
            let _ = started.send(result.as_ref().map(|_| config.bind_address).map_err(Clone::clone));
            result
        }
        address = listening_handle.listening() => {
            let address = address.ok_or_else(|| "remote server stopped before listening".to_owned())?;
            let _ = started.send(Ok(address));
            server.await.map_err(|error| error.to_string())
        }
    }
}

async fn upgrade_websocket(
    State(state): State<EndpointState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    websocket: WebSocketUpgrade,
) -> Response {
    if !state
        .authentication_attempts
        .allows(peer.ip(), StdInstant::now())
    {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            "authentication temporarily blocked",
        )
            .into_response();
    }
    let websocket = websocket.protocols([REMOTE_WEBSOCKET_PROTOCOL]);
    if websocket.selected_protocol().is_none() {
        return (StatusCode::BAD_REQUEST, "missing Lapis WebSocket protocol").into_response();
    }
    websocket
        .max_frame_size(state.limits.max_frame_size().bytes())
        .max_message_size(state.limits.max_message_size().bytes())
        .on_upgrade(move |socket| handle_socket(socket, state, peer))
}

async fn handle_socket(mut socket: WebSocket, state: EndpointState, peer: SocketAddr) {
    let mut connection = ConnectionGuard::new(
        ConnectionSession::new(),
        state.auth.clone(),
        state.handler.clone(),
    );
    let authentication_deadline =
        TokioInstant::now() + state.limits.authentication_timeout().duration();
    let mut idle_deadline = TokioInstant::now() + state.limits.idle_timeout().duration();
    let mut events: Option<RemoteEventReceiver> = None;

    loop {
        let deadline = if connection.session.is_authenticated() {
            idle_deadline
        } else {
            authentication_deadline
        };
        let incoming = if let Some(events) = events.as_mut() {
            tokio::select! {
                biased;
                received = timeout_at(deadline, socket.recv()) => match received {
                    Ok(Some(Ok(message))) => ConnectionInput::Message(message),
                    Ok(Some(Err(_)) | None) => ConnectionInput::SocketClosed,
                    Err(_) => ConnectionInput::Timeout,
                },
                event = events.recv() => match event {
                    Some(event) => ConnectionInput::Event(event),
                    None => ConnectionInput::EventsClosed,
                },
            }
        } else {
            match timeout_at(deadline, socket.recv()).await {
                Ok(Some(Ok(message))) => ConnectionInput::Message(message),
                Ok(Some(Err(_)) | None) => ConnectionInput::SocketClosed,
                Err(_) => ConnectionInput::Timeout,
            }
        };
        let message = match incoming {
            ConnectionInput::Message(message) => {
                idle_deadline = TokioInstant::now() + state.limits.idle_timeout().duration();
                message
            }
            ConnectionInput::Event(event) => {
                if send_message(
                    &mut socket,
                    ServerMessage::Event(event),
                    state.limits.request_timeout().duration(),
                )
                .await
                .is_err()
                {
                    break;
                }
                continue;
            }
            ConnectionInput::SocketClosed => break,
            ConnectionInput::EventsClosed => {
                let _ = close_socket(
                    &mut socket,
                    "event stream lagged; reconnect and request snapshot",
                    state.limits.request_timeout().duration(),
                )
                .await;
                break;
            }
            ConnectionInput::Timeout => {
                if !connection.session.is_authenticated() {
                    state
                        .authentication_attempts
                        .record_failure(peer.ip(), StdInstant::now());
                    let _ = send_message(
                        &mut socket,
                        protocol_error(UNAUTHORIZED),
                        state.limits.request_timeout().duration(),
                    )
                    .await;
                }
                let _ = close_socket(
                    &mut socket,
                    "connection timeout",
                    state.limits.request_timeout().duration(),
                )
                .await;
                break;
            }
        };

        let client_message = match message {
            Message::Text(text) => match serde_json::from_str::<ClientMessage>(text.as_str()) {
                Ok(message) => message,
                Err(_) => {
                    if !connection.session.is_authenticated() {
                        state
                            .authentication_attempts
                            .record_failure(peer.ip(), StdInstant::now());
                    }
                    let _ = send_message(
                        &mut socket,
                        protocol_error(PROTOCOL_ERROR),
                        state.limits.request_timeout().duration(),
                    )
                    .await;
                    let _ = close_socket(
                        &mut socket,
                        "invalid protocol message",
                        state.limits.request_timeout().duration(),
                    )
                    .await;
                    break;
                }
            },
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Close(_) => break,
            Message::Binary(_) => {
                if !connection.session.is_authenticated() {
                    state
                        .authentication_attempts
                        .record_failure(peer.ip(), StdInstant::now());
                }
                let _ = send_message(
                    &mut socket,
                    protocol_error(PROTOCOL_ERROR),
                    state.limits.request_timeout().duration(),
                )
                .await;
                let _ = close_socket(
                    &mut socket,
                    "binary messages are not supported",
                    state.limits.request_timeout().duration(),
                )
                .await;
                break;
            }
        };

        let was_authenticated = connection.session.is_authenticated();
        let mut reply = connection
            .session
            .receive(
                client_message,
                &state.auth,
                state.handler.as_ref(),
                state.limits.request_timeout().duration(),
            )
            .await;
        if !was_authenticated && connection.session.is_authenticated() {
            state.authentication_attempts.record_success(peer.ip());
            idle_deadline = TokioInstant::now() + state.limits.idle_timeout().duration();
        } else if !was_authenticated && reply.close {
            state
                .authentication_attempts
                .record_failure(peer.ip(), StdInstant::now());
        }
        if matches!(
            &reply.message,
            ServerMessage::Response(ResponseEnvelope {
                body: ResponseBody::WorkspaceConnect(_),
                ..
            })
        ) {
            let grant = connection
                .session
                .grant()
                .expect("workspace response requires an authenticated session")
                .clone();
            match timeout(
                state.limits.request_timeout().duration(),
                state.handler.subscribe(grant),
            )
            .await
            {
                Ok(Ok(receiver)) => events = receiver,
                Ok(Err(_)) | Err(_) => {
                    reply.message = subscription_error(&reply.message);
                    reply.close = true;
                }
            }
        }
        if send_message(
            &mut socket,
            reply.message,
            state.limits.request_timeout().duration(),
        )
        .await
        .is_err()
        {
            break;
        }
        if reply.close {
            let _ = close_socket(
                &mut socket,
                "request rejected",
                state.limits.request_timeout().duration(),
            )
            .await;
            break;
        }
    }

    connection.disconnect();
}

enum ConnectionInput {
    Message(Message),
    Event(EventEnvelope),
    SocketClosed,
    EventsClosed,
    Timeout,
}

fn subscription_error(message: &ServerMessage) -> ServerMessage {
    let request_id = match message {
        ServerMessage::Response(response) => response.request_id.clone(),
        _ => return protocol_error(INTERNAL),
    };
    ServerMessage::Response(ResponseEnvelope {
        request_id,
        body: ResponseBody::Error(ProtocolError::new(
            ErrorCode::try_new(INTERNAL).expect("built-in error code must be valid"),
        )),
    })
}

async fn send_message(
    socket: &mut WebSocket,
    message: ServerMessage,
    send_timeout: Duration,
) -> Result<(), ()> {
    let json = serde_json::to_string(&message).map_err(|_| ())?;
    timeout(send_timeout, socket.send(Message::Text(json.into())))
        .await
        .map_err(|_| ())?
        .map_err(|_| ())
}

async fn close_socket(
    socket: &mut WebSocket,
    reason: &'static str,
    send_timeout: Duration,
) -> Result<(), ()> {
    timeout(
        send_timeout,
        socket.send(Message::Close(Some(CloseFrame {
            code: close_code::POLICY,
            reason: reason.into(),
        }))),
    )
    .await
    .map_err(|_| ())?
    .map_err(|_| ())
}

struct ConnectionGuard {
    session: ConnectionSession,
    auth: SharedRemoteAuth,
    handler: Arc<dyn RemoteRequestHandler>,
}

impl ConnectionGuard {
    fn new(
        session: ConnectionSession,
        auth: SharedRemoteAuth,
        handler: Arc<dyn RemoteRequestHandler>,
    ) -> Self {
        Self {
            session,
            auth,
            handler,
        }
    }

    fn disconnect(&mut self) {
        self.session.disconnect(&self.auth, self.handler.as_ref());
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.disconnect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lapis_client_api::CapabilitySet;

    use crate::{
        AuthConfig, AuthPolicy, CredentialLifetime, PairingLifetime, RemoteAuth,
        RemoteResponseFuture,
    };

    struct UnusedHandler;

    impl RemoteRequestHandler for UnusedHandler {
        fn handle(
            &self,
            _session: crate::SessionGrant,
            _request: lapis_client_api::RequestEnvelope,
        ) -> RemoteResponseFuture<'_> {
            Box::pin(async { panic!("disabled server must not dispatch requests") })
        }
    }

    #[test]
    fn disabled_remote_cannot_start_listening() {
        let auth = test_auth();
        let auth = Arc::new(std::sync::Mutex::new(auth));
        let tls = test_tls_config();
        let result = RemoteServer::start(
            RemoteServerConfig::new("127.0.0.1:0".parse().unwrap(), tls, RemoteLimits::default()),
            auth,
            Arc::new(UnusedHandler),
        );

        assert!(matches!(result, Err(RemoteServerError::RemoteDisabled)));
    }

    #[test]
    fn enabled_server_reports_bound_address_and_shuts_down() {
        let mut auth = test_auth();
        auth.enable();
        let server = RemoteServer::start(
            RemoteServerConfig::new(
                "127.0.0.1:0".parse().unwrap(),
                test_tls_config(),
                RemoteLimits::default(),
            ),
            Arc::new(std::sync::Mutex::new(auth)),
            Arc::new(UnusedHandler),
        )
        .unwrap();

        assert!(server.listening_address().port() > 0);
        server.shutdown().unwrap();
    }

    fn test_auth() -> RemoteAuth {
        RemoteAuth::system(
            AuthConfig::new(
                PairingLifetime::new(10).unwrap(),
                CredentialLifetime::new(20).unwrap(),
            ),
            AuthPolicy::new(CapabilitySet::default()),
        )
    }

    fn test_tls_config() -> Tls13ServerConfig {
        use rustls::server::{ClientHello, ResolvesServerCert};

        #[derive(Debug)]
        struct NoCertificate;

        impl ResolvesServerCert for NoCertificate {
            fn resolve(
                &self,
                _client_hello: ClientHello<'_>,
            ) -> Option<Arc<rustls::sign::CertifiedKey>> {
                None
            }
        }

        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(NoCertificate));
        Tls13ServerConfig::from_test_config(config)
    }
}
