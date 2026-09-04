//! Remote接続の認証・認可と外部resource境界。

mod auth;
mod authorization;
mod backend_handler;
mod clock;
mod credential;
mod handler;
mod limits;
mod pairing;
mod random;
mod rate_limit;
mod secret;
mod server;
mod session;
mod tls;
mod wire;

pub use auth::{
    AccessError, AuthConfig, AuthConfigError, AuthError, AuthPolicy, CredentialLifetime,
    PairingLifetime, RemoteAuth,
};
pub use authorization::{AuthorizationError, SessionGrant};
pub use backend_handler::BackendRemoteHandler;
pub use clock::{Clock, SystemClock};
pub use credential::{CredentialHandle, CredentialId, InvalidCredentialId};
pub use handler::{
    RemoteEventReceiver, RemoteRequestHandler, RemoteResponseFuture, RemoteSubscriptionError,
    RemoteSubscriptionFuture,
};
pub use lapis_backend_state::{PathSecurityError, WorkspacePathResolver};
pub use limits::{
    AuthenticationTimeout, IdleTimeout, LimitError, MaxConcurrentRequests, MaxFrameSize,
    MaxMessageSize, RemoteLimits, RequestTimeout,
};
pub use pairing::PairingToken;
pub use random::{OsRandom, RandomError, RandomSource};
pub use rate_limit::{AuthenticationRateLimit, AuthenticationRateLimitError};
pub use server::{RemoteServer, RemoteServerConfig, RemoteServerError};
pub use session::SharedRemoteAuth;
pub use tls::{Tls13ServerConfig, tls13_server_config};
pub use wire::{
    AuthenticateRequest, ClientMessage, InvalidSecretHex, PairRequest, PairedResponse,
    REMOTE_WEBSOCKET_PATH, REMOTE_WEBSOCKET_PROTOCOL, ServerMessage,
};
