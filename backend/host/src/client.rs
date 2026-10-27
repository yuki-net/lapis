use crate::BackendHost;
use lapis_backend_state::{BackendEventReceiver, BackendServiceError, EmbeddedClient};
use lapis_client_api::{RequestBody, ResponseBody, WorkspaceId, WorkspaceRelativePath};
use lapis_remote::{QuicConnection, QuicConnectionConfig};
use std::{
    path::{Component, Path, PathBuf},
    sync::Arc,
};

/// 現在のbinaryと同じディレクトリから `lapis-host` binaryを解決する。
fn resolve_host_exe() -> Result<PathBuf, std::io::Error> {
    let current = std::env::current_exe()?;
    let mut dir = current
        .parent()
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "binaryの親ディレクトリが取得できません",
            )
        })?
        .to_path_buf();
    // cargo test のdeps/ディレクトリから実行される場合、target/debug/ に戻る。
    if dir.ends_with("deps") {
        if let Some(parent) = dir.parent() {
            dir = parent.to_path_buf();
        }
    }
    // Windowsでは.exe、Unixでは拡張子なし
    #[cfg(windows)]
    let candidate = dir.join("lapis-host.exe");
    #[cfg(not(windows))]
    let candidate = dir.join("lapis-host");
    if candidate.exists() {
        return Ok(candidate);
    }
    // 拡張子の逆パターンも試す
    #[cfg(windows)]
    let fallback = dir.join("lapis-host");
    #[cfg(not(windows))]
    let fallback = dir.join("lapis-host.exe");
    if fallback.exists() {
        return Ok(fallback);
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!(
            "lapis-host binaryが見つかりません: {} または {}",
            candidate.display(),
            fallback.display()
        ),
    ))
}

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
    pub fn local(executable: PathBuf, directory: PathBuf) -> Self {
        Self {
            source: Source::Local(Arc::new(crate::local::LocalHosts::new(
                executable, directory,
            ))),
        }
    }
    /// 現在のbinaryと同じディレクトリから `lapis-host` を解決し、Local Host を作成する。
    pub fn local_default() -> Result<Self, BackendServiceError> {
        let host_exe = resolve_host_exe().map_err(|_| BackendServiceError::Unavailable)?;
        Ok(Self::local(host_exe, Self::user_directory()?.join("hosts")))
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
        match &self.source {
            Source::Embedded(host) => host.register_workspace(root),
            Source::Local(hosts) => hosts
                .config(root)
                .map(|config| config.workspace_id)
                .map_err(|_| BackendServiceError::Unavailable),
            Source::Quic {
                root: expected,
                config,
            } if root == expected => Ok(config.workspace_id.clone()),
            _ => Err(BackendServiceError::Unavailable),
        }
    }
    pub(crate) fn open_worktree(
        &self,
        workspace: WorkspaceId,
        worktree: lapis_editor_core::TaskId,
    ) -> Result<PathBuf, BackendServiceError> {
        let path = match &self.source {
            Source::Embedded(host) => host.service().worktree_path(workspace, worktree)?,
            Source::Local(_) => {
                match self
                    .connect_workspace(workspace)?
                    .request(RequestBody::HostWorktree(worktree))?
                {
                    ResponseBody::HostWorktree(path) => PathBuf::from(path),
                    _ => return Err(BackendServiceError::Unavailable),
                }
            }
            // Remote credentialに管理権限や任意Workspaceへの接続権限を追加しない。
            Source::Quic { .. } => return Err(BackendServiceError::Unavailable),
        };
        self.register_workspace(&path)?;
        Ok(path)
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
            Source::Quic { root, config } => Ok(vec![(root.clone(), config.workspace_id.clone())]),
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
            _ => Err(BackendServiceError::Unavailable),
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
