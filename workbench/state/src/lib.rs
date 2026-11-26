use std::path::PathBuf;

pub use client_api::{
    HostExtensionCapabilitiesResponse, HostExtensionCapability, HostExtensionSetCapabilityRequest,
};
pub use document::Encoding;

mod connection_display;
mod extension_contributions;
#[cfg(any(target_arch = "wasm32", test))]
mod extension_management;
mod extensions;
mod process;
mod settings_store;
pub mod terminal_extension;
mod update;
mod view_state;
mod web_access;
mod web_control;
mod workbench_sync;
// Session 共有の適用判定は platform 共通。Desktop だけが背景 thread で受信する。
mod session_view;
pub use connection_display::{
    connected_workspace_name, connection_label, host_label, host_name_from_url,
    short_workspace_label, window_title, workspace_label,
};
pub use extension_contributions::{ExtensionContributionSession, ExtensionContributionSnapshot};
pub use extensions::{ExtensionDescriptor, ExtensionId, ExtensionRegistry, git_extension};
pub use process::ProcessSession;
pub use session_view::{SessionViewCursor, SessionViewReplica, SessionViewWatch};
#[cfg(not(target_arch = "wasm32"))]
pub use session_view::{SessionViewSession, SessionViewSync};
pub use settings_store::{SettingsModel, SettingsWatch};
pub use update::{UpdateController, UpdateStatus, UpdateWatch};
pub use view_state::{DocumentViewState, WorkspaceStateRepository, WorkspaceViewState};
pub use web_access::{
    PageScheme, WebAccess, WebAddress, WebAddressKind, address_kind_for, is_tailscale_interface,
    split_host_port, split_url,
};
pub use web_control::{HostDevice, HostWebStatus, WebAccessControl};
pub use workbench_sync::{WorkbenchSync, WorkbenchSyncResult, WorkbenchWatch};

#[cfg(any(target_arch = "wasm32", test))]
mod web_cache;
#[cfg(target_arch = "wasm32")]
pub mod web_host;
#[cfg(any(target_arch = "wasm32", test))]
#[path = "web_host/state.rs"]
mod web_replica;
#[cfg(any(target_arch = "wasm32", test))]
mod web_switch;

mod editor_session;
mod search_session;
mod settings_session;
mod task_session;
mod terminal_session;

pub use editor_session::{
    DocumentCloseFailure, DocumentCloseProgress, DocumentCloseStage, DocumentTab, EditorSession,
};
pub use search_session::WorkspaceSearchSession;
pub use settings_session::SettingsSession;
pub use task_session::TaskSession;
pub use terminal_session::{RestoredTerminal, TerminalInputFailure, TerminalSession};

#[cfg(test)]
mod tests;

/// ネイティブダイアログを利用するユースケース側の契約。
pub trait WorkspaceDialog: Send + Sync {
    /// Remoteではnative pickerを使わず、この名前空間内の相対pathを入力する。
    fn path_scope(&self) -> Option<PathBuf> {
        None
    }
    fn choose_workspace_path(&self) -> Option<PathBuf>;
    fn choose_file_path(&self) -> Option<PathBuf> {
        None
    }
    fn choose_save_path(&self, suggested_name: &str) -> Option<PathBuf>;
}

pub use workspace::WorkspaceRepository;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentAction {
    Completed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentCloseDisposition {
    PreserveChanges,
    DiscardChanges,
}

pub use workbench::{
    ConversationViewState, PanelLayoutViewState, SharedDocumentView, SharedWorkspaceView, TabRef,
    ViewBindingState, ViewScopeState, WorkbenchLayoutState, WorkbenchModeState, WorkbenchViewState,
    WorkspaceView, ZONE_LAYOUT_VERSION, ZoneLayoutViewState, ZonePanelViewState, ZoneViewState,
};

mod conversation;
pub use conversation::{
    ConversationRecord, ConversationRepository, ConversationSession, ConversationState,
};
pub use project::{Project, SourceRole, TaskEnvironment};

pub use extensions::git::{GitDiffSelection, GitSession};
