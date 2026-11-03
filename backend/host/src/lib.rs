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
mod workbench;
mod workspace;
mod workspace_registry;

/// Hostが提供する操作だけをペアリングで許可する。
pub fn supported_capabilities() -> lapis_client_api::CapabilitySet {
    use lapis_client_api::{CapabilityId, CapabilitySet, capability::*};
    CapabilitySet::try_new(
        [
            WORKSPACES,
            WORKSPACES_CONNECT,
            WORKSPACES_SWITCH,
            WORKSPACES_REGISTER,
            WORKSPACE_SYNC,
            WORKBENCH_WRITE,
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
    state_directory: Option<PathBuf>,
}

fn workspace_id_for_root(root: &Path) -> WorkspaceId {
    use sha2::{Digest, Sha256};
    let key = format!("{:x}", Sha256::digest(root.as_os_str().as_encoded_bytes()));
    WorkspaceId::try_new(format!("workspace-{key}")).expect("bounded workspace id")
}

impl BackendHost {
    pub fn workspaces(&self) -> Arc<dyn lapis_workspace::WorkspaceRepository> {
        BackendClient::from(self.clone()).workspaces()
    }
    pub fn documents(&self) -> Arc<dyn lapis_document::DocumentConnection> {
        BackendClient::from(self.clone()).documents()
    }
    pub fn start() -> Result<Self, BackendServiceError> {
        Self::start_inner(None)
    }

    pub fn start_with_state_directory(directory: PathBuf) -> Result<Self, BackendServiceError> {
        Self::start_inner(Some(directory))
    }

    fn start_inner(state_directory: Option<PathBuf>) -> Result<Self, BackendServiceError> {
        let state = BackendState::new([]).map_err(BackendServiceError::State)?;
        let host = Self {
            inner: Arc::new(HostInner {
                service: BackendService::start(state)?,
                workspaces: Mutex::new(HashMap::new()),
                next_session: AtomicU64::new(1),
                state_directory,
            }),
        };
        if let Some(directory) = &host.inner.state_directory {
            let records = workspace_registry::load(directory)
                .map_err(|_| BackendServiceError::Unavailable)?;
            for (expected, root) in records {
                let canonical = match root.canonicalize() {
                    Ok(root) => root,
                    Err(_) => {
                        eprintln!("保存されたWorkspaceを復元できません。登録情報は保持します。");
                        continue;
                    }
                };
                if workspace_id_for_root(&canonical) != expected {
                    eprintln!("保存されたWorkspaceの識別が一致しません。登録情報は保持します。");
                    continue;
                }
                host.register_workspace(&canonical)?;
            }
        }
        Ok(host)
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
            .cloned()
        {
            self.persist_workspace(root, &id)?;
            return Ok(id);
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
            self.persist_workspace(&root, workspace_id)?;
            return Ok(workspace_id.clone());
        }
        if !root.is_dir() {
            return Err(BackendServiceError::Unavailable);
        }
        let workspace_id = workspace_id_for_root(&root);
        let mut registration = WorkspaceRegistration::new(
            workspace_id.clone(),
            root.file_name().unwrap_or_default().to_string_lossy(),
            root.clone(),
            Arc::new(LocalWorkspaceRepository),
        )
        .with_terminal(Arc::new(LocalTerminalBackend::default()))
        .with_git(Arc::new(LocalGitBackend::user_default()))
        .with_tasks(Arc::new(LocalTaskBackend::user_default()));
        if let Some(directory) = &self.inner.state_directory {
            let path = directory
                .join("workbench")
                .join(format!("{}.json", workspace_id.as_str()));
            registration = registration.with_workbench_persistence(Arc::new(
                lapis_platform::LocalWorkbenchStore::new(path),
            ));
        }
        self.inner.service.register_workspace(registration)?;
        workspaces.insert(root.clone(), workspace_id.clone());
        self.persist_workspace(&root, &workspace_id)?;
        Ok(workspace_id)
    }

    fn persist_workspace(&self, root: &Path, id: &WorkspaceId) -> Result<(), BackendServiceError> {
        if let Some(directory) = &self.inner.state_directory {
            workspace_registry::save(directory, id, root)
                .map_err(|_| BackendServiceError::Unavailable)?;
        }
        Ok(())
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
