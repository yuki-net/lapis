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
pub use client::{BackendClient, ExtensionConnection};
#[cfg(windows)]
mod extension_pty;
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
            EXTENSIONS_MANAGE,
            EXTENSIONS_EXECUTE,
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
    extensions: Arc<crate::extensions::HostExtensions>,
    /// 拡張の有効状態と実行中のプロセスは Host 起動時に固定する。
    extension_host: Option<extension_runtime::supervisor::ExtensionSupervisor>,
    #[cfg(windows)]
    extension_pty: Arc<crate::extension_pty::PtyBroker>,
    extension_contributions: Vec<crate::extensions::StartupContribution>,
    extension_data_active: Vec<String>,
    extension_runtime_ids: Vec<String>,
    extension_startup_errors: std::collections::BTreeMap<String, String>,
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
        Self::start_inner(None, false)
    }

    pub fn start_with_state_directory(directory: PathBuf) -> Result<Self, BackendServiceError> {
        Self::start_inner(Some(directory), false)
    }

    /// ローカル開発に限り、署名のない拡張 package の導入と実行を許可する。
    pub fn start_development_with_state_directory(
        directory: PathBuf,
    ) -> Result<Self, BackendServiceError> {
        Self::start_inner(Some(directory), true)
    }

    fn start_inner(
        state_directory: Option<PathBuf>,
        development: bool,
    ) -> Result<Self, BackendServiceError> {
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
        let extensions = Arc::new(if development {
            crate::extensions::HostExtensions::load_with_development(
                state_directory.as_deref(),
                true,
            )
        } else {
            crate::extensions::HostExtensions::load(state_directory.as_deref())
        });
        let bundled_failure = if state_directory.is_some() {
            std::env::current_exe()
                .ok()
                .and_then(|exe| {
                    exe.parent().map(|parent| {
                        extensions.install_bundled(&parent.join("bundled-extensions"))
                    })
                })
                .and_then(Result::err)
        } else {
            None
        };
        let (
            components,
            extension_contributions,
            extension_data_active,
            extension_runtime_ids,
            mut extension_startup_errors,
        ) = extensions.startup_components();
        if let Some(error) = bundled_failure {
            extension_startup_errors.insert("bundled".to_owned(), error);
        }
        #[cfg(windows)]
        let extension_pty = Arc::new(crate::extension_pty::PtyBroker::new(Arc::clone(
            &extensions,
        )));
        let extension_host = if components.is_empty() {
            None
        } else {
            match extensions.worker_executable() {
                Some(path) => {
                    #[cfg(windows)]
                    let supervisor =
                        extension_runtime::supervisor::ExtensionSupervisor::start_with_handler(
                            path.to_path_buf(),
                            components,
                            Some(extension_pty.clone()),
                        );
                    #[cfg(not(windows))]
                    let supervisor = extension_runtime::supervisor::ExtensionSupervisor::start(
                        path.to_path_buf(),
                        components,
                    );
                    Some(supervisor)
                }
                None => {
                    extension_startup_errors.insert(
                        "host".to_owned(),
                        "拡張ホストの配置を解決できません".to_owned(),
                    );
                    None
                }
            }
        };
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
                extension_host,
                #[cfg(windows)]
                extension_pty,
                extension_contributions,
                extension_data_active,
                extension_runtime_ids,
                extension_startup_errors,
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

    /// 有効状態の設定とは別に、現在の拡張ホストの実行状態を返す。
    pub fn extension_host_status(&self) -> Option<extension_runtime::supervisor::SupervisorStatus> {
        self.inner.extension_host.as_ref().map(|host| host.status())
    }

    /// 呼び出し元の認可は transport が行う。実行中の契約宣言を必ず確認する。
    pub fn dispatch_extension(
        &self,
        request: &client_api::HostExtensionDispatchRequest,
        session: &client_api::SessionId,
        workspace: &client_api::WorkspaceId,
    ) -> Result<client_api::HostExtensionDispatchResponse, String> {
        if !self.inner.extensions.declares_dispatch(&request.id)? {
            return Err("拡張が dispatch 契約を宣言していません".into());
        }
        let host = self
            .inner
            .extension_host
            .as_ref()
            .ok_or("拡張ホストが停止しています")?;
        let context = extension_runtime::InvocationContext {
            session: session.as_str().to_owned(),
            workspace: workspace.as_str().to_owned(),
        };
        let result = host.dispatch_with_context(
            &request.id,
            &request.operation,
            &request.payload,
            &context,
        )?;
        Ok(client_api::HostExtensionDispatchResponse {
            id: request.id.clone(),
            operation: request.operation.clone(),
            result,
        })
    }

    /// 署名不正や旧形式など、起動時に検証できなかった拡張。
    pub fn extension_startup_errors(&self) -> &std::collections::BTreeMap<String, String> {
        &self.inner.extension_startup_errors
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
        let mut response = self.inner.extensions.list()?;
        let status = self.extension_host_status();
        let active = match &status {
            Some(extension_runtime::supervisor::SupervisorStatus::Running { active, .. }) => {
                active.clone()
            }
            _ => Vec::new(),
        };
        for entry in &mut response.extensions {
            entry.active = if self.inner.extension_data_active.contains(&entry.id) {
                Some(true)
            } else if self.inner.extension_runtime_ids.contains(&entry.id) {
                match &status {
                    Some(extension_runtime::supervisor::SupervisorStatus::Starting) => None,
                    _ => Some(active.contains(&entry.id)),
                }
            } else {
                Some(false)
            };
        }
        response.contributions = self
            .inner
            .extension_contributions
            .iter()
            .filter(|item| item.available(&active))
            .map(|item| item.payload.clone())
            .collect();
        Ok(response)
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

    pub fn extension_capabilities(
        &self,
        id: &str,
    ) -> Result<client_api::HostExtensionCapabilitiesResponse, String> {
        self.inner.extensions.capabilities(id)
    }

    pub fn set_extension_capability(
        &self,
        request: &client_api::HostExtensionSetCapabilityRequest,
    ) -> Result<client_api::HostExtensionSetCapabilityResponse, String> {
        if request.capability != "process.pty" || request.version != "1.0.0" {
            return Err("未対応の能力です".into());
        }
        #[cfg(windows)]
        self.inner.extension_pty.set_approval(
            &request.id,
            request.approved,
            &request.manifest_digest,
        )?;
        #[cfg(not(windows))]
        {
            if request.approved {
                return Err("この Host は PTY 能力を提供していません".into());
            }
            let id = extensions_entry::ExtensionPackageId::try_new(&request.id)
                .map_err(|error| error.to_string())?;
            let capability = extensions_entry::ContractId::try_new(&request.capability)
                .map_err(|error| error.to_string())?;
            self.inner.extensions.revoke_capability(&id, &capability)?;
        }
        Ok(client_api::HostExtensionSetCapabilityResponse {
            id: request.id.clone(),
            manifest_digest: request.manifest_digest.clone(),
            capability: request.capability.clone(),
            version: request.version.clone(),
            approved: request.approved,
        })
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
