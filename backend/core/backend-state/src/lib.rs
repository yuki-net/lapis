//! DesktopとRemote clientが共有するWorkspace・Document・Terminalの正規状態。

mod error;
mod events;
mod path;
mod ports;
mod service;
mod state;
mod terminal_state;

pub use error::BackendStateError;
pub use events::BackendEventReceiver;
pub use path::{PathSecurityError, WorkspacePathResolver};
pub use ports::{WorkspaceEntry, WorkspaceEntryKind, WorkspaceFileBackend};
pub use service::{BackendService, BackendServiceError};
pub use state::{BackendSession, BackendState, WorkspaceRegistration};
