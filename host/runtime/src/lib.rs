//! DesktopとRemote clientが共有するWorkspace・Document・Terminalの正規状態。

mod client;
mod credential;
mod device;
mod error;
mod events;
mod operations;
mod path;
mod ports;
mod service;
mod state;
mod terminal_state;
mod workbench;

pub use client::EmbeddedClient;
pub use credential::StoredCredential;
pub use device::{DeviceStatus, StoredDevice};
pub use error::BackendStateError;
pub use events::{BackendEventReceiver, BackendEventSink};
pub use path::{PathSecurityError, WorkspacePathResolver};
pub use ports::{WorkspaceEntry, WorkspaceEntryKind, WorkspaceFileBackend};
pub use service::{BackendService, BackendServiceError};
pub use state::{BackendSession, BackendState, WorkspaceRegistration};
pub use workbench::{WorkbenchPersistence, WorkbenchStoreResult};
