use crate::BackendHost;
use lapis_backend_state::{BackendEventReceiver, BackendServiceError, EmbeddedClient};
use lapis_client_api::{RequestBody, ResponseBody, WorkspaceId, WorkspaceRelativePath};
use lapis_remote::{QuicConnection, QuicConnectionConfig};
use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
};

/// Desktop向けportの変換を共有し、資源の所有者と接続先を分離する。
#[derive(Clone)]
pub struct BackendClient {
    source: Source,
}
#[derive(Clone)]
enum Source {
    Embedded(BackendHost),
    Local(Arc<crate::local::LocalHosts>),
    Quic {
        root: PathBuf,
        config: Arc<QuicConnectionConfig>,
        bindings: Arc<Mutex<HashMap<PathBuf, QuicConnectionConfig>>>,
        sources: Arc<std::sync::OnceLock<Vec<(PathBuf, String)>>>,
    },
}

#[derive(Clone)]
pub(crate) enum Client {
    Embedded(EmbeddedClient),
    Quic(QuicConnection),
}
impl Client {
    pub(crate) fn workspace_id(&self) -> &WorkspaceId {
        match self {
            Self::Embedded(client) => client.workspace_id(),
            Self::Quic(client) => client.workspace_id(),
        }
    }
    pub(crate) fn request(&self, body: RequestBody) -> Result<ResponseBody, BackendServiceError> {
        match self {
            Self::Embedded(client) => client.request(body),
            Self::Quic(client) => client.request(body),
        }
    }
    pub(crate) fn subscribe(&self) -> Result<BackendEventReceiver, BackendServiceError> {
        match self {
            Self::Embedded(client) => client.subscribe(),
            Self::Quic(client) => client.subscribe(),
        }
    }
}

impl From<BackendHost> for BackendClient {
    fn from(host: BackendHost) -> Self {
        Self {
            source: Source::Embedded(host),
        }
    }
}

impl BackendClient {
    pub(crate) fn open_worktree(
        &self,
        workspace: &lapis_editor_core::WorkspaceId,
        worktree: &lapis_editor_core::TaskId,
    ) -> Result<PathBuf, BackendServiceError> {
        match &self.source {
            Source::Embedded(host) => {
                let id = WorkspaceId::try_new(workspace.as_str())
                    .map_err(|_| BackendServiceError::Unavailable)?;
                let path = host.service().worktree_path(id, worktree.clone())?;
                host.register_workspace(&path)?;
                Ok(path)
            }
            source => {
                let id = WorkspaceId::try_new(workspace.as_str())
                    .map_err(|_| BackendServiceError::Unavailable)?;
                let root = self.root_for(&id)?;
                let mut config = match source {
                    Source::Local(hosts) => hosts
                        .by_id(&id)
                        .map_err(|_| BackendServiceError::Unavailable)?,
                    Source::Quic {
                        config, bindings, ..
                    } => {
                        if config.workspace_id == id {
                            config.as_ref().clone()
                        } else {
                            bindings
                                .lock()
                                .map_err(|_| BackendServiceError::Unavailable)?
                                .values()
                                .find(|config| config.workspace_id == id)
                                .cloned()
                                .ok_or(BackendServiceError::Unavailable)?
                        }
                    }
                    Source::Embedded(_) => unreachable!(),
                };
                let connection = QuicConnection::connect(config.clone())?;
                let response = connection.request(RequestBody::WorktreeOpen(
                    lapis_client_api::WorktreeOpenRequest {
                        workspace_id: id,
                        worktree_id: worktree.clone(),
                    },
                ))?;
                let ResponseBody::WorktreeOpen(opened) = response else {
                    return Err(match response {
                        ResponseBody::Error(error) => BackendServiceError::Rejected(error),
                        _ => BackendServiceError::Unavailable,
                    });
                };
                config.workspace_id = opened.workspace_id;
                let alias = root.join(".lapis-worktrees").join(worktree.as_str());
                match source {
                    Source::Local(hosts) => hosts
                        .bind(alias.clone(), config)
                        .map_err(|_| BackendServiceError::Unavailable)?,
                    Source::Quic { bindings, .. } => {
                        bindings
                            .lock()
                            .map_err(|_| BackendServiceError::Unavailable)?
                            .insert(alias.clone(), config);
                    }
                    Source::Embedded(_) => unreachable!(),
                }
                Ok(alias)
            }
        }
    }
    pub fn local(executable: PathBuf, directory: PathBuf) -> Self {
        Self {
            source: Source::Local(Arc::new(crate::local::LocalHosts::new(
                executable, directory,
            ))),
        }
    }
    pub fn local_default() -> Result<Self, BackendServiceError> {
        Ok(Self::local(
            std::env::current_exe().map_err(|_| BackendServiceError::Unavailable)?,
            Self::user_directory()?.join("hosts"),
        ))
    }
    pub fn user_directory() -> Result<PathBuf, BackendServiceError> {
        let base = std::env::var_os("LOCALAPPDATA")
            .or_else(|| std::env::var_os("XDG_STATE_HOME"))
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
            })
            .ok_or(BackendServiceError::Unavailable)?;
        Ok(base.join("Lapis"))
    }
    pub(crate) fn default_root(&self) -> Result<PathBuf, BackendServiceError> {
        match &self.source {
            Source::Embedded(_) | Source::Local(_) => {
                std::env::current_dir().map_err(|_| BackendServiceError::Unavailable)
            }
            Source::Quic { root, .. } => Ok(root.clone()),
        }
    }
    /// rootはClient内だけの表示上の名前空間。RemoteのOS pathとしては使用しない。
    pub fn remote(
        root: PathBuf,
        config: QuicConnectionConfig,
    ) -> Result<Self, BackendServiceError> {
        if !root.is_absolute()
            || root
                .components()
                .any(|part| matches!(part, Component::ParentDir))
        {
            return Err(BackendServiceError::Unavailable);
        }
        Ok(Self {
            source: Source::Quic {
                root,
                config: Arc::new(config),
                bindings: Default::default(),
                sources: Default::default(),
            },
        })
    }
    pub fn workspaces(&self) -> Arc<dyn lapis_workspace::WorkspaceRepository> {
        Arc::new(crate::workspace::HostWorkspaces::new(self.clone()))
    }
    pub fn documents(&self) -> Arc<dyn lapis_document::DocumentConnection> {
        Arc::new(crate::document::HostDocuments::new(self.clone()))
    }
    pub fn git(&self) -> Arc<dyn lapis_git::GitConnection> {
        Arc::new(crate::features::HostGit::new(self.clone()))
    }
    pub fn tasks(&self) -> Arc<dyn lapis_task::TaskConnection> {
        Arc::new(crate::features::HostTasks::new(self.clone()))
    }
    pub fn terminals(&self) -> Arc<dyn lapis_terminal::TerminalBackend> {
        Arc::new(crate::terminal::HostTerminals::new(self.clone()))
    }

    pub fn register_workspace(&self, root: &Path) -> Result<WorkspaceId, BackendServiceError> {
        if !matches!(&self.source, Source::Embedded(_)) {
            if let Some((_, id)) = self.roots()?.into_iter().find(|(known, _)| known == root) {
                return Ok(id);
            }
            if root
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == ".lapis-sources")
            {
                self.project_sources()?;
                return self
                    .roots()?
                    .into_iter()
                    .find(|(known, _)| known == root)
                    .map(|(_, id)| id)
                    .ok_or(BackendServiceError::Unavailable);
            }
            if let Some(parent) = root.parent()
                && parent
                    .file_name()
                    .is_some_and(|name| name == ".lapis-worktrees")
                && let (Some(source), Some(id)) = (
                    parent.parent(),
                    root.file_name().and_then(|name| name.to_str()),
                )
            {
                let workspace = self.register_workspace(source)?;
                let opened = self.open_worktree(
                    &lapis_editor_core::WorkspaceId::new(workspace.as_str()),
                    &lapis_editor_core::TaskId::new(id),
                )?;
                return self
                    .roots()?
                    .into_iter()
                    .find(|(known, _)| known == &opened)
                    .map(|(_, id)| id)
                    .ok_or(BackendServiceError::Unavailable);
            }
        }
        match &self.source {
            Source::Embedded(host) => host.register_workspace(root),
            Source::Local(hosts) => hosts
                .config(root)
                .map(|config| config.workspace_id)
                .map_err(|_| BackendServiceError::Unavailable),
            Source::Quic {
                root: expected,
                config,
                ..
            } if root == expected => Ok(config.workspace_id.clone()),
            _ => Err(BackendServiceError::Unavailable),
        }
    }
    pub(crate) fn roots(&self) -> Result<Vec<(PathBuf, WorkspaceId)>, BackendServiceError> {
        match &self.source {
            Source::Embedded(host) => Ok(host
                .inner
                .workspaces
                .lock()
                .map_err(|_| BackendServiceError::Unavailable)?
                .iter()
                .map(|(root, id)| (root.clone(), id.clone()))
                .collect()),
            Source::Quic {
                root,
                config,
                bindings,
                ..
            } => {
                let mut roots = vec![(root.clone(), config.workspace_id.clone())];
                roots.extend(
                    bindings
                        .lock()
                        .map_err(|_| BackendServiceError::Unavailable)?
                        .iter()
                        .map(|(root, config)| (root.clone(), config.workspace_id.clone())),
                );
                Ok(roots)
            }
            Source::Local(hosts) => hosts.roots().map_err(|_| BackendServiceError::Unavailable),
        }
    }
    pub(crate) fn root_for(&self, workspace: &WorkspaceId) -> Result<PathBuf, BackendServiceError> {
        self.roots()?
            .into_iter()
            .find(|(_, id)| id == workspace)
            .map(|(root, _)| root)
            .ok_or(BackendServiceError::Unavailable)
    }

    pub(crate) fn project_sources(&self) -> Result<Vec<(PathBuf, String)>, BackendServiceError> {
        let Source::Quic {
            root,
            config,
            bindings,
            sources,
        } = &self.source
        else {
            return Ok(Vec::new());
        };
        if let Some(sources) = sources.get() {
            return Ok(sources.clone());
        }
        let connection = QuicConnection::connect(config.as_ref().clone())?;
        let response = connection.request(RequestBody::WorkspaceList(Default::default()))?;
        let ResponseBody::WorkspaceList(catalog) = response else {
            return Err(match response {
                ResponseBody::Error(error) => BackendServiceError::Rejected(error),
                _ => BackendServiceError::Unavailable,
            });
        };
        let mut available = Vec::new();
        let mut bindings = bindings
            .lock()
            .map_err(|_| BackendServiceError::Unavailable)?;
        for source in catalog.workspaces {
            let alias = if source.workspace_id == config.workspace_id {
                root.clone()
            } else {
                root.join(".lapis-sources")
                    .join(source.workspace_id.as_str())
            };
            let mut binding = config.as_ref().clone();
            binding.workspace_id = source.workspace_id;
            bindings.insert(alias.clone(), binding);
            available.push((alias, source.name));
        }
        available.sort_by(|left, right| left.0.cmp(&right.0));
        let _ = sources.set(available.clone());
        Ok(available)
    }
    pub(crate) fn connect(&self, root: &Path) -> Result<Client, BackendServiceError> {
        self.connect_workspace(self.register_workspace(root)?)
    }
    pub(crate) fn connect_workspace(
        &self,
        workspace: WorkspaceId,
    ) -> Result<Client, BackendServiceError> {
        match &self.source {
            Source::Embedded(host) => host.connect_workspace(workspace).map(Client::Embedded),
            Source::Local(hosts) => QuicConnection::connect(
                hosts
                    .by_id(&workspace)
                    .map_err(|_| BackendServiceError::Unavailable)?,
            )
            .map(Client::Quic),
            Source::Quic { config, .. } if config.workspace_id == workspace => {
                QuicConnection::connect(config.as_ref().clone()).map(Client::Quic)
            }
            Source::Quic { bindings, .. } => {
                let config = bindings
                    .lock()
                    .map_err(|_| BackendServiceError::Unavailable)?
                    .values()
                    .find(|config| config.workspace_id == workspace)
                    .cloned()
                    .ok_or(BackendServiceError::Unavailable)?;
                QuicConnection::connect(config).map(Client::Quic)
            }
        }
    }
    pub(crate) fn directory(
        &self,
        directory: &Path,
    ) -> Result<(PathBuf, Option<WorkspaceRelativePath>), BackendServiceError> {
        let path = self.normalize_path(directory)?;
        let root = self
            .roots()?
            .into_iter()
            .map(|(root, _)| root)
            .filter(|root| path.starts_with(root))
            .max_by_key(|root| root.components().count())
            .unwrap_or(path.clone());
        let relative = path
            .strip_prefix(&root)
            .map_err(|_| BackendServiceError::Unavailable)?;
        let relative = if relative.as_os_str().is_empty() {
            None
        } else {
            Some(
                WorkspaceRelativePath::parse(
                    relative
                        .to_str()
                        .ok_or(BackendServiceError::Unavailable)?
                        .replace('\\', "/"),
                )
                .map_err(|_| BackendServiceError::Unavailable)?,
            )
        };
        Ok((root, relative))
    }
    pub(crate) fn connect_directory(
        &self,
        directory: &Path,
    ) -> Result<(Client, Option<WorkspaceRelativePath>), BackendServiceError> {
        let (root, relative) = self.directory(directory)?;
        Ok((self.connect(&root)?, relative))
    }
    pub(crate) fn normalize_path(&self, path: &Path) -> Result<PathBuf, BackendServiceError> {
        match &self.source {
            Source::Embedded(_) => path
                .canonicalize()
                .map_err(|_| BackendServiceError::Unavailable),
            Source::Local(_) => {
                if let Ok(path) = path.canonicalize() {
                    return Ok(path);
                }
                let path = if path.is_absolute() {
                    path.to_owned()
                } else {
                    std::env::current_dir()
                        .map_err(|_| BackendServiceError::Unavailable)?
                        .join(path)
                };
                if path
                    .components()
                    .any(|part| matches!(part, Component::ParentDir))
                {
                    return Err(BackendServiceError::Unavailable);
                }
                if self.roots()?.iter().any(|(root, _)| path.starts_with(root)) {
                    Ok(path)
                } else {
                    Err(BackendServiceError::Unavailable)
                }
            }
            Source::Quic { root, .. } => {
                let path = if path.is_absolute() {
                    path.to_owned()
                } else {
                    root.join(path)
                };
                if !path.starts_with(root)
                    || path
                        .components()
                        .any(|part| matches!(part, Component::ParentDir))
                {
                    return Err(BackendServiceError::Unavailable);
                }
                Ok(path)
            }
        }
    }
    pub(crate) fn normalize_file(&self, path: &Path) -> Result<PathBuf, BackendServiceError> {
        match &self.source {
            Source::Quic { .. } => self.normalize_path(path),
            Source::Embedded(_) | Source::Local(_) => match path.canonicalize() {
                Ok(path) => Ok(path),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    let name = path.file_name().ok_or(BackendServiceError::Unavailable)?;
                    let parent = path
                        .parent()
                        .filter(|path| !path.as_os_str().is_empty())
                        .unwrap_or(Path::new("."));
                    Ok(self.normalize_path(parent)?.join(name))
                }
                Err(_) => Err(BackendServiceError::Unavailable),
            },
        }
    }
    pub(crate) fn relocate_document(
        &self,
        source: WorkspaceId,
        destination: WorkspaceId,
        document: lapis_client_api::DocumentId,
        revision: lapis_client_api::Revision,
        path: WorkspaceRelativePath,
    ) -> Result<(), BackendServiceError> {
        match &self.source {
            Source::Embedded(host) => {
                host.service()
                    .relocate_document(source, destination, document, revision, path)
            }
            Source::Local(_) => {
                let client = self.connect_workspace(source)?;
                match client.request(RequestBody::DocumentRelocate(
                    lapis_client_api::DocumentRelocateRequest {
                        document_id: document,
                        destination_workspace: destination,
                        base_revision: revision,
                        path,
                    },
                ))? {
                    ResponseBody::DocumentRelocate(_) => Ok(()),
                    ResponseBody::Error(problem) => Err(BackendServiceError::Rejected(problem)),
                    _ => Err(BackendServiceError::Unavailable),
                }
            }
            Source::Quic { .. } => Err(BackendServiceError::Unavailable),
        }
    }
}
