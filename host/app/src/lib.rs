//! 実行資源を組み立て、接続とは独立して Runtime を所有する Host。
mod client;
mod local;
mod oidc;
mod profile;
mod remote_handler;
pub use remote_handler::HostRemoteHandler;
mod identity;
pub use identity::HostIdentity;
mod protected;
pub use local::run_managed_host;
pub use oidc::{HostOidc, configured as oidc_configured, from_env as oidc_from_env};
pub use profile::{
    HostInvitation, HostProfileInfo, OidcProfile, host_profile_info, open_host_file,
    read_invitation, write_invitation, write_invitation_replacing, write_oidc_profile,
};
mod document;
pub use client::BackendClient;
mod extensions;
mod features;
mod session_view;
mod terminal;
mod web_publisher;
mod workbench;
mod workspace;
/// Hostが提供する操作だけをペアリングで許可する。
pub fn supported_capabilities() -> client_api::CapabilitySet {
    use client_api::{CapabilityId, CapabilitySet, capability::*};
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
            "workspace.process.read",
            "workspace.process.control",
            EXTENSIONS_READ,
        ]
        .map(|name| CapabilityId::try_new(name).expect("built-in capability")),
    )
    .expect("bounded capabilities")
}

use ::terminal::{TerminalBackend, TerminalProfiles};
use backend_state::{
    BackendService, BackendServiceError, BackendState, EmbeddedClient, WorkspaceRegistration,
};
use client_api::{SessionId, WorkspaceId};
use platform::{LocalGitBackend, LocalTaskBackend, LocalTerminalBackend, LocalWorkspaceRepository};
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
    store: Option<Mutex<platform::WorkspaceStore>>,
    /// Host が起動を許可する Terminal profile。
    terminal_profiles: TerminalProfiles,
    /// 導入済み拡張の目録。state directory が無い Host は持たない。
    extensions: crate::extensions::HostExtensions,
    /// Host 自身と Host が spawn したプロセスのレジストリ。
    processes: Arc<dyn process::ProcessRegistry>,
}

impl HostInner {
    /// パスから永続 ID を得る。state directory が無い場合はパスから導出する。
    fn workspace_id(&self, root: &Path) -> Result<WorkspaceId, BackendServiceError> {
        let Some(store) = &self.store else {
            return Ok(workspace_id_for_root(root));
        };
        let store = store.lock().map_err(|_| BackendServiceError::Unavailable)?;
        let record = store
            .register(root)
            .map_err(|_| BackendServiceError::Unavailable)?;
        WorkspaceId::try_new(record.id).map_err(|_| BackendServiceError::Unavailable)
    }
}

/// Host の Terminal profile 設定を読む。無い場合は OS 既定を使う。
fn load_terminal_profiles(directory: Option<&Path>) -> TerminalProfiles {
    let Some(directory) = directory else {
        return platform::default_terminal_profiles();
    };
    let path = directory.join("terminal-profiles.json");
    let Ok(bytes) = std::fs::read(&path) else {
        return platform::default_terminal_profiles();
    };
    match serde_json::from_slice::<TerminalProfiles>(&bytes) {
        Ok(profiles) if !profiles.profiles.is_empty() => profiles,
        Ok(_) => {
            eprintln!(
                "Terminal profile設定が空のため、Terminalを起動できません: {}",
                path.display()
            );
            TerminalProfiles::default()
        }
        Err(error) => {
            eprintln!(
                "Terminal profile設定を読めないため、Terminalを起動できません: {}: {error}",
                path.display()
            );
            TerminalProfiles::default()
        }
    }
}

/// state directory が無い場合の暫定 ID。パスから導出し、再起動しても同じ値になる。
fn workspace_id_for_root(root: &Path) -> WorkspaceId {
    use sha2::{Digest, Sha256};
    let key = format!("{:x}", Sha256::digest(root.as_os_str().as_encoded_bytes()));
    WorkspaceId::try_new(format!("workspace-{key}")).expect("bounded workspace id")
}

impl remote::WorkspaceResolver for BackendHost {
    fn host_id(&self) -> Option<String> {
        BackendHost::host_id(self)
    }

    fn primary_workspace(&self) -> Option<WorkspaceId> {
        self.default_workspace()
    }

    fn contains(&self, workspace_id: &WorkspaceId) -> bool {
        self.workspace_exists(workspace_id)
    }
}

impl BackendHost {
    pub fn workspaces(&self) -> Arc<dyn ::workspace::WorkspaceRepository> {
        BackendClient::from(self.clone()).workspaces()
    }
    pub fn documents(&self) -> Arc<dyn ::document::DocumentConnection> {
        BackendClient::from(self.clone()).documents()
    }
    /// Workspace の view state を Session 間で共有する接続。
    #[cfg(not(target_arch = "wasm32"))]
    pub fn session_view(&self) -> Arc<dyn ::workbench::SessionViewConnection> {
        BackendClient::from(self.clone()).session_view()
    }
    pub fn start() -> Result<Self, BackendServiceError> {
        Self::start_inner(None)
    }

    pub fn start_with_state_directory(directory: PathBuf) -> Result<Self, BackendServiceError> {
        Self::start_inner(Some(directory))
    }

    fn start_inner(state_directory: Option<PathBuf>) -> Result<Self, BackendServiceError> {
        let state = BackendState::new([]).map_err(BackendServiceError::State)?;
        let store = match &state_directory {
            Some(directory) => {
                std::fs::create_dir_all(directory).map_err(|_| BackendServiceError::Unavailable)?;
                let path = directory.join("workspaces.sqlite3");
                Some(Mutex::new(
                    platform::WorkspaceStore::open(&path)
                        .map_err(|_| BackendServiceError::Unavailable)?,
                ))
            }
            None => None,
        };
        let terminal_profiles = load_terminal_profiles(state_directory.as_deref());
        let extensions = crate::extensions::HostExtensions::load(state_directory.as_deref());
        let processes = local_process::LocalProcessRegistry::new(process::ProcessOrigin::Host);
        let host = Self {
            inner: Arc::new(HostInner {
                service: BackendService::start(state)?,
                workspaces: Mutex::new(HashMap::new()),
                next_session: AtomicU64::new(1),
                state_directory,
                store,
                terminal_profiles,
                extensions,
                processes,
            }),
        };
        if let Some(store) = &host.inner.store {
            let records = store
                .lock()
                .map_err(|_| BackendServiceError::Unavailable)?
                .list()
                .map_err(|_| BackendServiceError::Unavailable)?;
            for record in records {
                let canonical = match record.root.canonicalize() {
                    Ok(root) => root,
                    Err(_) => {
                        eprintln!("保存されたWorkspaceを復元できません。登録情報は保持します。");
                        continue;
                    }
                };
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
            return Ok(workspace_id.clone());
        }
        if !root.is_dir() {
            return Err(BackendServiceError::Unavailable);
        }
        let workspace_id = self.inner.workspace_id(&root)?;
        let mut registration = WorkspaceRegistration::new(
            workspace_id.clone(),
            root.file_name().unwrap_or_default().to_string_lossy(),
            root.clone(),
            Arc::new(LocalWorkspaceRepository::new(root.clone())),
        )
        .with_terminal(Arc::new(LocalTerminalBackend::with_profiles(
            self.inner.terminal_profiles.clone(),
        )))
        .with_git(Arc::new(LocalGitBackend::user_default()))
        .with_tasks(Arc::new(LocalTaskBackend::user_default().with_processes(
            self.inner.processes.clone(),
            editor_core::WorkspaceId::new(workspace_id.as_str()),
        )))
        .with_processes(self.inner.processes.clone());
        if let Some(directory) = &self.inner.state_directory {
            let path = directory
                .join("workbench")
                .join(format!("{}.json", workspace_id.as_str()));
            registration = registration
                .with_workbench_persistence(Arc::new(platform::LocalWorkbenchStore::new(path)));
        }
        self.inner.service.register_workspace(registration)?;
        workspaces.insert(root.clone(), workspace_id.clone());
        Ok(workspace_id)
    }

    /// 接続先 Workspace を指定しなかった Client に提示する既定 Workspace。
    pub fn default_workspace(&self) -> Option<WorkspaceId> {
        if let Some(store) = &self.inner.store
            && let Ok(store) = store.lock()
            && let Ok(Some(record)) = store.primary()
            && let Ok(id) = WorkspaceId::try_new(record.id)
        {
            return Some(id);
        }
        let workspaces = self.inner.workspaces.lock().ok()?;
        workspaces
            .values()
            .min_by(|left, right| left.as_str().cmp(right.as_str()))
            .cloned()
    }

    /// Host の永続的な同一性。state directory が無い場合は `None`。
    pub fn host_id(&self) -> Option<String> {
        let store = self.inner.store.as_ref()?;
        let store = store.lock().ok()?;
        store.host_id().ok()
    }

    /// Workspace ID が Host に登録されているか。
    pub fn workspace_exists(&self, workspace_id: &WorkspaceId) -> bool {
        if let Some(store) = &self.inner.store
            && let Ok(store) = store.lock()
            && let Ok(records) = store.list()
        {
            return records
                .iter()
                .any(|record| record.id == workspace_id.as_str());
        }
        self.inner
            .workspaces
            .lock()
            .map(|workspaces| workspaces.values().any(|known| known == workspace_id))
            .unwrap_or(false)
    }

    /// 既定 Workspace へ接続する。
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

    pub fn git(&self) -> Arc<dyn git::GitConnection> {
        BackendClient::from(self.clone()).git()
    }
    pub fn tasks(&self) -> Arc<dyn task::TaskConnection> {
        BackendClient::from(self.clone()).tasks()
    }
    pub fn terminals(&self) -> Arc<dyn TerminalBackend> {
        BackendClient::from(self.clone()).terminals()
    }
    pub fn processes(&self) -> Arc<dyn process::ProcessConnection> {
        BackendClient::from(self.clone()).processes()
    }

    /// 導入済み拡張の目録を宣言として返す。Host の state directory が正本。
    pub fn extensions(&self) -> Result<client_api::HostExtensionsListResponse, String> {
        self.inner.extensions.list()
    }

    /// `.lapis` package を読み込み、目録へ宣言を登録する。
    ///
    /// Host の管理操作であり、リモートの Client には公開しない。反映（有効な拡張の
    /// 読み込み）は入口の再起動で行う。
    pub fn install_extension(
        &self,
        package: &Path,
        enabled: bool,
    ) -> Result<extensions_entry::InstallOutcome, String> {
        self.inner.extensions.install(package, enabled)
    }

    /// 拡張を有効化・無効化する。反映は入口の再起動で行う。
    pub fn set_extension_enabled(
        &self,
        id: &str,
        enabled: bool,
    ) -> Result<Option<extensions_entry::Registration>, String> {
        self.inner.extensions.set_enabled(id, enabled)
    }

    /// 導入済み package を取り除く。拡張データは残す。
    pub fn remove_extension(&self, id: &str) -> Result<(), String> {
        self.inner.extensions.remove(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_profiles_fall_back_only_when_the_setting_is_absent() {
        let directory = tempfile::tempdir().unwrap();
        assert!(
            !load_terminal_profiles(Some(directory.path()))
                .profiles
                .is_empty()
        );

        let path = directory.path().join("terminal-profiles.json");
        std::fs::write(
            &path,
            r#"{"profiles":[{"id":"custom","name":"Custom","program":"/bin/custom"}],"default":"custom"}"#,
        )
        .unwrap();
        let profiles = load_terminal_profiles(Some(directory.path()));
        assert_eq!(profiles.profiles.len(), 1);
        assert_eq!(profiles.profiles[0].id.as_str(), "custom");

        std::fs::write(&path, "not json").unwrap();
        assert!(
            load_terminal_profiles(Some(directory.path()))
                .profiles
                .is_empty()
        );
    }
}
