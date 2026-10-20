//! 実行資源を組み立て、接続とは独立して Runtime を所有する Host。
mod document;
mod features;
mod terminal;
mod workspace;

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
        Arc::new(workspace::HostWorkspaces::new(self.clone()))
    }
    pub fn documents(&self) -> Arc<dyn lapis_document::DocumentConnection> {
        Arc::new(document::HostDocuments::new(self.clone()))
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
        let workspace_id = WorkspaceId::try_new(format!("workspace-{}", workspaces.len() + 1))
            .expect("bounded workspace id");
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
        let sequence = self.inner.next_session.fetch_add(1, Ordering::Relaxed);
        let session_id =
            SessionId::try_new(format!("desktop-{sequence}")).expect("bounded session id");
        EmbeddedClient::connect(self.service(), session_id, workspace_id)
    }

    pub(crate) fn connect_directory(
        &self,
        directory: &Path,
    ) -> Result<
        (
            EmbeddedClient,
            Option<lapis_client_api::WorkspaceRelativePath>,
        ),
        BackendServiceError,
    > {
        let directory = directory
            .canonicalize()
            .map_err(|_| BackendServiceError::Unavailable)?;
        let root = {
            let workspaces = self
                .inner
                .workspaces
                .lock()
                .map_err(|_| BackendServiceError::Unavailable)?;
            workspaces
                .keys()
                .filter(|root| directory.starts_with(root))
                .max_by_key(|root| root.components().count())
                .cloned()
        }
        .unwrap_or_else(|| directory.clone());
        let relative = directory
            .strip_prefix(&root)
            .map_err(|_| BackendServiceError::Unavailable)?;
        let relative = if relative.as_os_str().is_empty() {
            None
        } else {
            Some(
                lapis_client_api::WorkspaceRelativePath::parse(
                    relative
                        .to_str()
                        .ok_or(BackendServiceError::Unavailable)?
                        .replace('\\', "/"),
                )
                .map_err(|_| BackendServiceError::Unavailable)?,
            )
        };
        Ok((self.connect(&root)?, relative))
    }

    pub fn git(&self) -> Arc<dyn lapis_git::GitConnection> {
        Arc::new(features::HostGit::new(self.clone()))
    }
    pub fn tasks(&self) -> Arc<dyn lapis_task::TaskConnection> {
        Arc::new(features::HostTasks::new(self.clone()))
    }
    pub fn terminals(&self) -> Arc<dyn TerminalBackend> {
        Arc::new(terminal::HostTerminals::new(self.clone()))
    }
}
