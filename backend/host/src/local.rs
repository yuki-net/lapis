use crate::{BackendHost, protected::protect};
use lapis_client_api::*;
use lapis_remote::{
    AuthConfig, AuthPolicy, CredentialHandle, CredentialId, CredentialLifetime, PairingLifetime,
    QuicConnection, QuicConnectionConfig, QuicServer, RemoteAuth, RemoteLimits,
    tls13_server_config,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};
use zeroize::{Zeroize, Zeroizing};

type Error = Box<dyn std::error::Error + Send + Sync>;

#[derive(Serialize, Deserialize)]
pub(super) struct Record {
    pub(super) version: u32,
    pub(super) key: String,
    pub(super) pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) started_at: Option<String>,
    #[serde(default)]
    pub(super) managed_lease: bool,
    pub(super) address: std::net::SocketAddr,
    pub(super) server_name: String,
    pub(super) label: String,
    pub(super) certificate: Vec<u8>,
    pub(super) workspace: String,
    pub(super) client: String,
    pub(super) credential: String,
    pub(super) secret: Vec<u8>,
}
impl Drop for Record {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}
impl Record {
    pub(super) fn config(&self) -> Result<QuicConnectionConfig, Error> {
        if self.version != 1 || self.secret.len() != 32 {
            return Err("Local Host情報が不正です".into());
        }
        let mut secret = [0; 32];
        secret.copy_from_slice(&self.secret);
        Ok(QuicConnectionConfig {
            address: self.address,
            server_name: self.server_name.clone(),
            certificate: self.certificate.clone().into(),
            hello: hello(&self.client)?,
            workspace_id: WorkspaceId::try_new(&self.workspace)?,
            credential: Arc::new(CredentialHandle::from_parts(
                CredentialId::parse(&self.credential)?,
                &mut secret,
            )),
        })
    }
}
fn hello(client: &str) -> Result<ClientHello, Error> {
    Ok(ClientHello {
        protocol: ProtocolRange::exact(CURRENT_PROTOCOL),
        client_id: ClientId::try_new(client)?,
        client_name: "Desktop".to_owned(),
        client_kind: ClientKind::Desktop,
        requested_capabilities: crate::supported_capabilities(),
    })
}
const LOCAL_KEY: &str = "local-v1";

pub(crate) struct LocalHosts {
    executable: PathBuf,
    directory: PathBuf,
    configs: Mutex<HashMap<PathBuf, QuicConnectionConfig>>,
}
impl LocalHosts {
    pub(crate) fn new(executable: PathBuf, directory: PathBuf) -> Self {
        Self {
            executable,
            directory,
            configs: Mutex::new(HashMap::new()),
        }
    }
    pub(crate) fn roots(&self) -> Result<Vec<(PathBuf, WorkspaceId)>, Error> {
        Ok(self
            .configs
            .lock()
            .map_err(|_| "Host一覧が利用できません")?
            .iter()
            .map(|(root, config)| (root.clone(), config.workspace_id.clone()))
            .collect())
    }
    pub(crate) fn by_id(&self, id: &WorkspaceId) -> Result<QuicConnectionConfig, Error> {
        self.configs
            .lock()
            .map_err(|_| "Host一覧が利用できません")?
            .values()
            .find(|config| &config.workspace_id == id)
            .cloned()
            .ok_or_else(|| "Workspaceが登録されていません".into())
    }
    pub(crate) fn config(&self, root: &Path) -> Result<QuicConnectionConfig, Error> {
        let mut configs = self
            .configs
            .lock()
            .map_err(|_| "Host一覧が利用できません")?;
        if let Some(config) = configs.get(root) {
            return Ok(config.clone());
        }
        let root = root.canonicalize().or_else(|error| {
            if root.is_absolute()
                && !root
                    .components()
                    .any(|part| matches!(part, std::path::Component::ParentDir))
            {
                Ok(root.to_owned())
            } else {
                Err(error)
            }
        })?;
        if let Some(config) = configs.get(&root) {
            return Ok(config.clone());
        }
        fs::create_dir_all(&self.directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.directory, fs::Permissions::from_mode(0o700))?;
        }
        let key = LOCAL_KEY;
        let lock_path = self.directory.join(format!("{key}.lock"));
        let lock = private_options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        lock.lock()?;
        let record_path = self.directory.join(format!("{key}.host"));
        if record_path.exists() {
            let mut bytes = Zeroizing::new(Vec::new());
            fs::File::open(&record_path)?
                .take(128 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() > 128 * 1024 {
                return Err("Host情報の上限を超えています".into());
            }
            let plaintext = protect(&bytes, key.as_bytes(), true)?;
            let record: Record = serde_json::from_slice(&plaintext)?;
            if record.key != key || !record.address.ip().is_loopback() {
                return Err("Host情報がWorkspaceと一致しません".into());
            }
            // 通信失敗だけではHost停止とみなさない。生存中のHostを二重起動しない。
            if host_record_active(&record, &self.directory)? {
                let config = register(record.config()?, &root)?;
                configs.insert(root, config.clone());
                return Ok(config);
            }
        }
        if !root.is_dir() {
            return Err("Workspaceのフォルダが存在しません".into());
        }
        let mut command = Command::new(&self.executable);
        command
            .arg("--managed-host")
            .arg(&root)
            .arg(&self.directory)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn()?;
        let pid = child.id();
        let stdout = child
            .stdout
            .take()
            .ok_or("Hostの起動情報を取得できません")?;
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("lapis-host-bootstrap".to_owned())
            .spawn(move || {
                let mut line = Zeroizing::new(String::new());
                let result = BufReader::new(stdout)
                    .take(128 * 1024)
                    .read_line(&mut line)
                    .map(|_| line);
                let _ = sender.send(result);
            })?;
        let record = (|| {
            let line = receiver
                .recv_timeout(Duration::from_secs(30))
                .map_err(|_| "Hostの起動がtimeoutしました")??;
            let record: Record = serde_json::from_str(&line)?;
            if record.pid != pid || record.key != key || !record.address.ip().is_loopback() {
                return Err::<Record, Error>("Hostの起動情報が一致しません".into());
            }
            if !record.managed_lease
                || record.started_at.is_none()
                || !record_process_matches(&record)?
            {
                return Err("Hostのprocess開始情報が一致しません".into());
            }
            record.config()?;
            let plaintext = Zeroizing::new(serde_json::to_vec(&record)?);
            let encrypted = protect(&plaintext, key.as_bytes(), false)?;
            let mut temp = tempfile::NamedTempFile::new_in(&self.directory)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                temp.as_file()
                    .set_permissions(fs::Permissions::from_mode(0o600))?;
            }
            temp.write_all(&encrypted)?;
            temp.as_file().sync_all()?;
            temp.persist(&record_path)?;
            Ok(record)
        })();
        let record = match record {
            Ok(record) => record,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        let config = register(record.config()?, &root)?;
        configs.insert(root, config.clone());
        // ChildのDropは終了要求ではない。HostはDesktop終了後も独立して動作する。
        Ok(config)
    }
}

fn private_options() -> OpenOptions {
    let options = OpenOptions::new();
    #[cfg(unix)]
    let mut options = options;
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options
}

fn record_process_matches(record: &Record) -> io::Result<bool> {
    let Some(actual) = process_started_at(record.pid)? else {
        return Ok(false);
    };
    // 旧形式は同一性を証明できないため、通信失敗を理由に置換しない。
    match &record.started_at {
        Some(expected) => Ok(expected == &actual),
        None => Err(io::Error::other(
            "旧Local Hostの開始情報がありません。Hostを終了してから再起動してください",
        )),
    }
}

fn host_record_active(record: &Record, directory: &Path) -> io::Result<bool> {
    if !record.managed_lease {
        return record_process_matches(record);
    }
    let lease = private_options()
        .read(true)
        .write(true)
        .open(directory.join("local-v1.lease"))?;
    match lease.try_lock() {
        Ok(()) => Ok(false),
        Err(std::fs::TryLockError::WouldBlock) => Ok(true),
        Err(std::fs::TryLockError::Error(error)) => Err(error),
    }
}

#[cfg(windows)]
fn process_started_at(pid: u32) -> io::Result<Option<String>> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, FILETIME, GetLastError, STILL_ACTIVE},
        System::Threading::{
            GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        },
    };
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        if unsafe { GetLastError() } == ERROR_INVALID_PARAMETER {
            return Ok(None);
        }
        return Err(io::Error::last_os_error());
    }
    let mut code = 0;
    let mut started: FILETIME = unsafe { std::mem::zeroed() };
    let mut exited = started;
    let mut kernel = started;
    let mut user = started;
    let result = (|| {
        if unsafe { GetExitCodeProcess(process, &mut code) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if code != STILL_ACTIVE as u32 {
            return Ok(None);
        }
        if unsafe { GetProcessTimes(process, &mut started, &mut exited, &mut kernel, &mut user) }
            == 0
        {
            return Err(io::Error::last_os_error());
        }
        if exited.dwHighDateTime != 0 || exited.dwLowDateTime != 0 {
            return Ok(None);
        }
        Ok(Some(format!(
            "{}",
            (u64::from(started.dwHighDateTime) << 32) | u64::from(started.dwLowDateTime)
        )))
    })();
    unsafe {
        CloseHandle(process);
    }
    result
}
#[cfg(target_os = "linux")]
fn process_started_at(pid: u32) -> io::Result<Option<String>> {
    if pid == 0 {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let stat = match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    // commには空白や括弧を含められるため、最後の閉じ括弧以降のfieldを読む。
    let fields = stat.rsplit_once(')').ok_or(io::ErrorKind::InvalidData)?.1;
    if matches!(fields.split_whitespace().next(), Some("Z" | "X" | "x")) {
        return Ok(None);
    }
    let start = fields
        .split_whitespace()
        .nth(19)
        .ok_or(io::ErrorKind::InvalidData)?;
    let start: u64 = start.parse().map_err(|_| io::ErrorKind::InvalidData)?;
    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
    Ok(Some(format!("{}:{start}", boot.trim())))
}

#[cfg(target_os = "macos")]
fn process_started_at(pid: u32) -> io::Result<Option<String>> {
    let pid: libc::pid_t = pid.try_into().map_err(|_| io::ErrorKind::InvalidInput)?;
    if pid <= 0 {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of_val(&info) as i32;
    let read = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    if read == size {
        return Ok(Some(format!(
            "{}:{}",
            info.pbi_start_tvsec, info.pbi_start_tvusec
        )));
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(None)
    } else {
        Err(error)
    }
}

/// 親が作った匿名pipeだけへbootstrap情報を返す。secretを引数・ログへ渡さない。
pub fn run_managed_host(root: &Path, directory: &Path) -> Result<(), Error> {
    // Host自身が終了まで所有する。PIDが他ユーザーへ再利用されても、このleaseはOSが解放する。
    let lease = private_options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join("local-v1.lease"))?;
    lease.try_lock()?;
    let root = root.canonicalize()?;
    let host = BackendHost::start()?;
    let workspace = host.register_workspace(&root)?;
    let certified = rcgen::generate_simple_self_signed(vec!["lapis-host".to_owned()])?;
    let tls = tls13_server_config(
        vec![certified.cert.der().clone()],
        rustls::pki_types::PrivateKeyDer::Pkcs8(certified.signing_key.serialize_der().into()),
    )?;
    let owner_capabilities =
        CapabilitySet::try_new(crate::supported_capabilities().iter().cloned().chain(
            std::iter::once(CapabilityId::try_new(capability::HOST_MANAGE)?),
        ))?;
    let mut auth = RemoteAuth::system(
        AuthConfig::new(
            PairingLifetime::new(300)?,
            // Local所有者の鍵はOSユーザーとこのHostの寿命に紐付く。
            // Remoteへ配布する招待・credentialの期限とは分ける。
            CredentialLifetime::new(u64::MAX)?,
        ),
        AuthPolicy::new(owner_capabilities.clone()),
    );
    auth.enable();
    let mut random = [0u8; 16];
    getrandom::fill(&mut random)?;
    let client = format!(
        "desktop-{}",
        random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let token = auth.begin_pairing(workspace.clone())?;
    let credential =
        auth.complete_pairing(&token, &ClientId::try_new(&client)?, &owner_capabilities)?;
    let auth = Arc::new(Mutex::new(auth));
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let server = QuicServer::bind(
                "127.0.0.1:0".parse()?,
                tls,
                auth,
                Arc::new(crate::HostRemoteHandler::new(host.clone())),
                RemoteLimits::default(),
            )
            .await?;
            let record = Record {
                version: 1,
                key: LOCAL_KEY.to_owned(),
                pid: std::process::id(),
                started_at: Some(
                    process_started_at(std::process::id())?.ok_or("Host processが存在しません")?,
                ),
                managed_lease: true,
                address: server.listening_address()?,
                server_name: "lapis-host".to_owned(),
                label: root
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                certificate: certified.cert.der().to_vec(),
                workspace: workspace.as_str().to_owned(),
                client,
                credential: credential.id().as_str().to_owned(),
                secret: credential.secret_bytes().to_vec(),
            };
            serde_json::to_writer(io::stdout().lock(), &record)?;
            println!();
            io::stdout().flush()?;
            drop(record);
            tokio::signal::ctrl_c().await?;
            server.shutdown().await;
            Ok::<_, Error>(())
        })
}

fn register(mut config: QuicConnectionConfig, root: &Path) -> Result<QuicConnectionConfig, Error> {
    let connection = QuicConnection::connect(config.clone())?;
    let response = connection.request(RequestBody::HostRegister(HostRegisterRequest {
        root: root
            .to_str()
            .ok_or("WorkspaceのpathをUTF-8で表現できません")?
            .to_owned(),
    }))?;
    let ResponseBody::HostRegister(workspace) = response else {
        return Err("HostへのWorkspace登録が拒否されました".into());
    };
    config.workspace_id = workspace.workspace_id;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> Record {
        Record {
            version: 1,
            key: LOCAL_KEY.to_owned(),
            pid: std::process::id(),
            started_at: process_started_at(std::process::id()).unwrap(),
            managed_lease: false,
            address: "127.0.0.1:1".parse().unwrap(),
            server_name: "lapis-host".to_owned(),
            label: "test".to_owned(),
            certificate: Vec::new(),
            workspace: "test".to_owned(),
            client: "test".to_owned(),
            credential: "00000000000000000000000000000000".to_owned(),
            secret: vec![0; 32],
        }
    }

    #[test]
    fn reused_pid_is_not_the_recorded_host() {
        let mut record = record();
        assert!(record.started_at.is_some());
        assert!(record_process_matches(&record).unwrap());
        record.started_at = Some("previous-process".to_owned());
        assert!(!record_process_matches(&record).unwrap());
        record.started_at = None;
        assert!(record_process_matches(&record).is_err());
    }

    #[test]
    fn process_owned_lease_recovers_without_querying_a_reused_pid() {
        let directory = tempfile::tempdir().unwrap();
        let lease = private_options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(directory.path().join("local-v1.lease"))
            .unwrap();
        let mut record = record();
        record.managed_lease = true;
        record.pid = u32::MAX;
        lease.lock().unwrap();
        assert!(host_record_active(&record, directory.path()).unwrap());
        drop(lease);
        assert!(!host_record_active(&record, directory.path()).unwrap());
    }

    #[test]
    fn only_a_stale_process_identity_allows_bootstrap() {
        let state = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let path = state.path().join("local-v1.host");
        let mut record = record();
        let hosts = LocalHosts::new(
            state.path().join("missing-executable"),
            state.path().to_owned(),
        );
        record.config().unwrap();
        let save = |record: &Record| {
            let plaintext = Zeroizing::new(serde_json::to_vec(record).unwrap());
            let encrypted = protect(&plaintext, LOCAL_KEY.as_bytes(), false).unwrap();
            fs::write(&path, &encrypted).unwrap();
            encrypted
        };
        let original = save(&record);
        assert!(hosts.config(root.path()).is_err());
        assert_eq!(fs::read(&path).unwrap().as_slice(), original.as_slice());
        record.started_at = Some("previous-process".to_owned());
        let stale = save(&record);
        let error = hosts.config(root.path()).err().unwrap();
        assert_eq!(
            error.downcast_ref::<io::Error>().unwrap().kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(fs::read(&path).unwrap().as_slice(), stale.as_slice());
    }

    #[test]
    fn terminated_process_has_no_live_identity() {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .arg("--list")
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(process_started_at(pid).unwrap().is_none());
    }
}
