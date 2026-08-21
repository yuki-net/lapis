//! DesktopとMobileが共有する、transport非依存のclient契約。

mod connection;
mod handshake;
mod path;
mod version;

pub mod capability;

pub use capability::{CapabilityId, CapabilitySet};
pub use connection::ConnectionState;
pub use handshake::{ClientHello, ClientKind, ServerHello};
pub use path::{InvalidWorkspacePath, WorkspaceRelativePath};
pub use version::{
    CURRENT_PROTOCOL, InvalidProtocolRange, ProtocolRange, ProtocolVersion, VersionMismatch,
};
