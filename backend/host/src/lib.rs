//! 実行資源を組み立て、接続とは独立して Runtime を所有する Host。
mod client;
mod local;
mod profile;
mod remote_handler;
pub use remote_handler::HostRemoteHandler;
mod protected;
pub use local::run_managed_host;
pub use profile::{HostInvitation, open_host_file, write_invitation};
mod document;
pub use client::BackendClient;
mod features;
mod terminal;
mod workspace;

/// Hostが提供する操作だけをペアリングで許可する。
pub fn supported_capabilities() -> lapis_client_api::CapabilitySet {
    use lapis_client_api::{CapabilityId, CapabilitySet, capability::*};
    CapabilitySet::try_new(
        [
            WORKSPACES,
            WORKSPACES_CONNECT,
            WORKSPACE_SYNC,
            FILES_READ,
            FILES_WRITE,
            DOCUMENTS_READ,
            DOCUMENTS_WRITE,
            TERMINAL_START,
            TERMINAL_CONTROL,
            "workspace.git.read",
            "workspace.git.write",
            "workspace.tasks.read",
            "workspace.tasks.start",
            "workspace.tasks.control",
        ]
        .map(|name| CapabilityId::try_new(name).expect("built-in capability")),
    )
    .expect("bounded capabilities")
}

use lapis_backend_state::{
    BackendService, BackendServiceError, BackendState, EmbeddedClient, WorkspaceRegistration,
};
use lapis_client_api::{SessionId, WorkspaceId};
use lapis_platform::{
    LocalGitBackend, LocalTaskBackend, LocalTerminalBackend, LocalWorkspaceRepository,
};
use lapis_terminal::TerminalBackend;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

#[derive(Clone)]
pub struct BackendHost {
    inner: Arc<HostInner>,
}

struct HostInner {
    service: BackendService,
    workspaces: Mutex<HashMap<PathBuf, WorkspaceId>>,
    next_session: AtomicU64,
}

impl BackendHost {
    pub fn workspaces(&self) -> Arc<dyn lapis_workspace::WorkspaceRepository> {
        BackendClient::from(self.clone()).workspaces()
    }
    pub fn documents(&self) -> Arc<dyn lapis_document::DocumentConnection> {
        BackendClient::from(self.clone()).documents()
    }
    pub fn start() -> Result<Self, BackendServiceError> {
        let state = BackendState::new([]).map_err(BackendServiceError::State)?;
        Ok(Self {
            inner: Arc::new(HostInner {
                service: BackendService::start(state)?,
                workspaces: Mutex::new(HashMap::new()),
                next_session: AtomicU64::new(1),
            }),
        })
    }

    pub fn service(&self) -> BackendService {
        self.inner.service.clone()
    }

    pub fn register_workspace(&self, root: &Path) -> Result<WorkspaceId, BackendServiceError> {
        if let Some(id) = self
            .inner
            .workspaces
            .lock()
            .map_err(|_| BackendServiceError::Unavailable)?
            .get(root)
        {
            return Ok(id.clone());
        }
        let root = root
            .canonicalize()
            .map_err(|_| BackendServiceError::Unavailable)?;
        let mut workspaces = self
            .inner
            .workspaces
            .lock()
            .map_err(|_| BackendServiceError::Unavailable)?;
        if let Some(workspace_id) = workspaces.get(&root) {
            return Ok(workspace_id.clone());
        }
        if !root.is_dir() {
            return Err(BackendServiceError::Unavailable);
        }
        use sha2::{Digest, Sha256};
        let key = format!("{:x}", Sha256::digest(root.as_os_str().as_encoded_bytes()));
        let workspace_id =
            WorkspaceId::try_new(format!("workspace-{key}")).expect("bounded workspace id");
        let registration = WorkspaceRegistration::new(
            workspace_id.clone(),
            root.file_name().unwrap_or_default().to_string_lossy(),
            root.clone(),
            Arc::new(LocalWorkspaceRepository),
        )
        .with_terminal(Arc::new(LocalTerminalBackend::default()))
        .with_git(Arc::new(LocalGitBackend::user_default()))
        .with_tasks(Arc::new(LocalTaskBackend::user_default()));
        self.inner.service.register_workspace(registration)?;
        workspaces.insert(root, workspace_id.clone());
        Ok(workspace_id)
    }

    pub fn connect(&self, root: &Path) -> Result<EmbeddedClient, BackendServiceError> {
        let workspace_id = self.register_workspace(root)?;
        self.connect_workspace(workspace_id)
    }

    pub(crate) fn connect_workspace(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<EmbeddedClient, BackendServiceError> {
        let sequence = self.inner.next_session.fetch_add(1, Ordering::Relaxed);
        let session_id =
            SessionId::try_new(format!("desktop-{sequence}")).expect("bounded session id");
        EmbeddedClient::connect(self.service(), session_id, workspace_id)
    }

    pub fn git(&self) -> Arc<dyn lapis_git::GitConnection> {
        BackendClient::from(self.clone()).git()
    }
    pub fn tasks(&self) -> Arc<dyn lapis_task::TaskConnection> {
        BackendClient::from(self.clone()).tasks()
    }
    pub fn terminals(&self) -> Arc<dyn TerminalBackend> {
        BackendClient::from(self.clone()).terminals()
    }
}
