//! Remote接続の認可と外部resource境界。

mod authorization;
mod workspace_path;

pub use authorization::{AuthorizationError, SessionGrant};
pub use workspace_path::{PathSecurityError, WorkspacePathResolver};
