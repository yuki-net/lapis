//! DesktopとRemote clientが共有するWorkspace・Document・Terminalの正規状態。

mod client;
mod error;
mod events;
mod operations;
mod service;
mod state;

pub use auth_state::{DeviceStatus, StoredCredential, StoredDevice};
pub use client::EmbeddedClient;
pub use error::BackendStateError;
pub use events::{BackendEventReceiver, BackendEventSink};
pub use service::{BackendService, BackendServiceError};
pub use state::{BackendSession, BackendState, WorkspaceRegistration};
pub use workbench_state::{WorkbenchPersistence, WorkbenchStateError, WorkbenchStoreResult};
pub use workspace_state::{
    FileMetadata, PathSecurityError, WorkspaceEntry, WorkspaceEntryKind, WorkspaceFileBackend,
    WorkspacePathResolver,
};
