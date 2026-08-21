//! Remote接続の認証・認可と外部resource境界。

mod auth;
mod authorization;
mod clock;
mod credential;
mod pairing;
mod random;
mod secret;
mod workspace_path;

pub use auth::{
    AccessError, AuthConfig, AuthConfigError, AuthError, AuthPolicy, CredentialLifetime,
    PairingLifetime, RemoteAuth,
};
pub use authorization::{AuthorizationError, SessionGrant};
pub use clock::{Clock, SystemClock};
pub use credential::{CredentialHandle, CredentialId, InvalidCredentialId};
pub use pairing::PairingToken;
pub use random::{OsRandom, RandomError, RandomSource};
pub use workspace_path::{PathSecurityError, WorkspacePathResolver};
