//! DesktopとRemote clientが共有するWorkspace・Document・Terminalの正規状態。

mod client;
mod credential;
mod device;
mod error;
mod events;
mod operations;
mod service;
mod state;

pub use client::EmbeddedClient;
pub use credential::StoredCredential;
pub use device::{DeviceStatus, StoredDevice};
pub use error::BackendStateError;
pub use events::{BackendEventReceiver, BackendEventSink};
pub use service::{BackendService, BackendServiceError};
pub use state::{BackendSession, BackendState, WorkspaceRegistration};
pub use workbench_state::{WorkbenchPersistence, WorkbenchStateError, WorkbenchStoreResult};
pub use workspace_state::{
    PathSecurityError, WorkspaceEntry, WorkspaceEntryKind, WorkspaceFileBackend,
    WorkspacePathResolver,
};
