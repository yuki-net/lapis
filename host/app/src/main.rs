use host::BackendHost;
use platform::CredentialStore;
use remote::{
    AuthConfig, AuthPolicy, CredentialId, CredentialLifetime, PairingLifetime, QuicServer,
    RemoteAuth, RemoteLimits, tls13_server_config,
};
use std::{
    io::{BufRead, Write},
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use zeroize::Zeroizing;

type Error = Box<dyn std::error::Error + Send + Sync>;

/// 実質無期限。Hostの再起動後も同じURLで接続できるようにする。
const CREDENTIAL_LIFETIME_SECONDS: u64 = 100 * 365 * 24 * 60 * 60;
const CREDENTIAL_SAVE_INTERVAL: Duration = Duration::from_secs(5);
const CREDENTIAL_DB: &str = "credentials.sqlite3";

struct Options {
    workspace: PathBuf,
    certificate: PathBuf,
    listen: SocketAddr,
    state_directory: Option<PathBuf>,
    connection_info: Option<PathBuf>,
    invitation: Option<PathBuf>,
    replace_certificate: bool,
}

impl Options {
    fn parse(arguments: impl Iterator<Item = std::ffi::OsString>) -> Result<Self, Error> {
        let mut arguments = arguments;
        let mut workspace = None;
        let mut certificate = None;
        let mut listen = "127.0.0.1:4787".parse()?;
        let mut state_directory = None;
        let mut connection_info = None;
        let mut invitation = None;
        let mut replace_certificate = false;
        while let Some(argument) = arguments.next() {
            if argument == "--replace-certificate" {
                replace_certificate = true;
                continue;
            }
            let value = arguments.next().ok_or("option requires a value")?;
            match argument.to_str() {
                Some("--workspace") => workspace = Some(PathBuf::from(value)),
                Some("--certificate") => certificate = Some(PathBuf::from(value)),
                Some("--listen") => listen = value.to_str().ok_or("invalid address")?.parse()?,
                Some("--state-directory") => state_directory = Some(PathBuf::from(value)),
                Some("--connection-info") => connection_info = Some(PathBuf::from(value)),
                Some("--invitation") => invitation = Some(PathBuf::from(value)),
                _ => return Err("unknown option".into()),
            }
        }
        Ok(Self {
            workspace: workspace.ok_or("--workspace is required")?,
            certificate: certificate.ok_or("--certificate is required")?,
            listen,
            state_directory,
            connection_info,
            invitation,
            replace_certificate,
        })
    }
}

fn main() {
    let arguments = std::env::args_os().collect::<Vec<_>>();
    if arguments
        .get(1)
        .is_some_and(|value| value == "--managed-host")
    {
        let result = arguments
            .get(2)
            .ok_or("Workspaceがありません")
            .map_err(|error| -> Error { error.into() })
            .and_then(|root| {
                let directory = arguments.get(3).ok_or("Hostの状態フォルダがありません")?;
                host::run_managed_host(std::path::Path::new(root), std::path::Path::new(directory))
            });
        if result.is_err() {
            std::process::exit(1);
        }
        return;
    }
    if arguments
        .get(1)
        .is_some_and(|value| value == "--task-worker")
    {
        let result = arguments
            .get(2)
            .ok_or("Task worker spec is required")
            .map_err(|message| -> Error { message.into() })
            .and_then(|path| {
                platform::run_task_worker(std::path::Path::new(path)).map_err(Into::into)
            });
        if let Err(error) = result {
            eprintln!("Task worker: {error}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(error) = run() {
        eprintln!("Host: {error}");
        eprintln!(
            "Usage: lapis-host --workspace <directory> --certificate <public.der> [--replace-certificate] [--listen <ip:port>] [--state-directory <directory>] [--connection-info <file>] [--invitation <file>]"
        );
        std::process::exit(1);
    }
}

fn run() -> Result<(), Error> {
    let options = Options::parse(std::env::args_os().skip(1))?;
    let state_directory = options.state_directory.clone();
    // 証明書と秘密鍵は state directory に保存し、再起動しても同じ身元で待ち受ける。
    let identity = host::HostIdentity::load_or_create(state_directory.as_deref())?;
    let host = match options.state_directory {
        Some(directory) => BackendHost::start_with_state_directory(directory)?,
        None => BackendHost::start()?,
    };
    let credentials_path = state_directory.map(|directory| directory.join(CREDENTIAL_DB));
    let workspace = host.register_workspace(&options.workspace)?;
    let tls = tls13_server_config(vec![identity.certificate.clone()], identity.key.clone_key())?;
    let mut auth = RemoteAuth::system(
        AuthConfig::new(
            PairingLifetime::new(3600)?,
            CredentialLifetime::new(CREDENTIAL_LIFETIME_SECONDS)?,
        )
        .with_auto_approve_oidc_devices(auto_approve_oidc_devices()),
        AuthPolicy::new(host::supported_capabilities()),
    );
    auth.set_workspace_resolver(Arc::new(host.clone()));
    let auth = Arc::new(Mutex::new(auth));
    if let Some(oidc) = host::oidc_from_env()? {
        auth.lock()
            .map_err(|_| "authentication unavailable")?
            .enable_oidc(oidc.verifier, oidc.client);
    }
    let mut credential_store = match &credentials_path {
        Some(path) => match CredentialStore::open(path) {
            Ok(store) => {
                let credentials = store.load();
                let devices = store.load_devices();
                match (credentials, devices) {
                    (Ok(records), Ok(devices)) => {
                        let (loaded_credentials, loaded_devices) = {
                            let mut auth = auth.lock().map_err(|_| "authentication unavailable")?;
                            auth.import_credentials(records);
                            let loaded_devices = devices.len();
                            auth.import_devices(devices);
                            (auth.export_credentials().len(), loaded_devices)
                        };
                        println!(
                            "Loaded {loaded_credentials} credential(s) and {loaded_devices} device(s) from {}",
                            path.display()
                        );
                        Some(store)
                    }
                    (Err(error), _) | (_, Err(error)) => {
                        eprintln!("Ignoring invalid auth database: {error}");
                        None
                    }
                }
            }
            Err(error) => {
                eprintln!("Credential database unavailable: {error}");
                None
            }
        },
        None => None,
    };
    auth.lock()
        .map_err(|_| "authentication unavailable")?
        .enable();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    // Tokioのstdinは取消不能なblocking readを持つため、Ctrl+C時にruntimeの終了を
    // 妨げない専用threadで読む。EOFはClient切断と同様、Host終了の理由にしない。
    let (input_sender, mut input) = tokio::sync::mpsc::channel(16);
    std::thread::Builder::new()
        .name("lapis-host-console".to_owned())
        .spawn(move || {
            for line in std::io::stdin().lock().lines() {
                if input_sender.blocking_send(line).is_err() {
                    break;
                }
            }
        })?;
    runtime.block_on(async {
        let server = QuicServer::bind(options.listen, tls, auth.clone(),
            Arc::new(host::HostRemoteHandler::new(host.clone())), RemoteLimits::default()).await?;
        // listenに失敗した起動が稼働中Hostの証明書を置き換えない。
        write_public_certificate(&options.certificate, identity.certificate.as_ref(), options.replace_certificate)?;
        let address = server.listening_address()?;
        if let Some(path) = &options.connection_info {
            let connection = wire::HostConnection {
                version: wire::HOST_CONNECTION_VERSION,
                address,
                server_name: "lapis-host".to_owned(),
                workspace_id: workspace.as_str().to_owned(),
                certificate: identity.certificate.as_ref().to_vec(),
            };
            write_connection_info(path, &connection)?;
        }
        println!("address={address} server_name=lapis-host workspace={}", workspace.as_str());
        // Desktop用の招待は1回限りのtokenを含むため、起動のたびに作り直す。
        if let Some(path) = &options.invitation {
            let token = auth
                .lock()
                .map_err(|_| "authentication unavailable")?
                .begin_pairing(workspace.clone())?;
            let invitation = host::HostInvitation {
                version: 1,
                address,
                server_name: "lapis-host".to_owned(),
                certificate: identity.certificate.as_ref().to_vec(),
                workspace_id: workspace.as_str().to_owned(),
                name: options
                    .workspace
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                token: token.secret_bytes().to_vec(),
            };
            host::write_invitation_replacing(path, &invitation)?;
            println!("invitation={}", path.display());
        }
        println!("Commands: pair / pair-user / register <directory> / invite <address:port> <new-file.json> / devices / approve <device-id> / revoke-device <device-id> / revoke <credential-id> / ext-install <file.lapis> [--disabled] / ext-enable <id> / ext-disable <id> / ext-remove <id> / quit");
        std::io::stdout().flush()?;
        let mut credential_save = tokio::time::interval(CREDENTIAL_SAVE_INTERVAL);
        credential_save.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut saved_credentials = None;
        let mut input_open = true;
        loop {
            tokio::select! {
                signal = tokio::signal::ctrl_c() => { signal?; break; }
                _ = credential_save.tick() => {
                    save_credentials(
                        credential_store.as_mut(),
                        &auth,
                        &mut saved_credentials,
                    )?;
                }
                line = input.recv(), if input_open => {
                    let Some(line) = line else { input_open = false; continue };
                    let line = line?;
                    let line = line.trim();
                    if line == "quit" { break; }
                    if line == "pair" || line == "pair-user" {
                        let mut auth = auth.lock().map_err(|_| "authentication unavailable")?;
                        let token = if line == "pair-user" {
                            auth.begin_user_session_pairing(workspace.clone())?
                        } else {
                            auth.begin_pairing(workspace.clone())?
                        };
                        drop(auth);
                        // 利用者が要求したローカル招待だけを表示し、ログへ自動出力しない。
                        let mut invitation = Zeroizing::new(String::with_capacity(64));
                        for byte in token.secret_bytes() {
                            use std::fmt::Write;
                            write!(invitation, "{byte:02x}")?;
                        }
                        if line == "pair-user" {
                            println!("User session pairing token (1 hour, one use): {}", invitation.as_str());
                        } else {
                            println!("Pairing token (1 hour, one use): {}", invitation.as_str());
                        }
                    } else if let Some(path) = line.strip_prefix("register ") {
                        let path = path.trim();
                        if path.is_empty() {
                            eprintln!("Usage: register <directory>");
                        } else {
                            match host.register_workspace(std::path::Path::new(path)) {
                                Ok(id) => println!("workspace={}", id.as_str()),
                                Err(_) => eprintln!("Workspaceを登録できません"),
                            }
                        }
                    } else if let Some(input) = line.strip_prefix("invite ") {
                        let mut parts = input.splitn(2, char::is_whitespace);
                        let address = parts.next().and_then(|value| value.parse().ok());
                        let path = parts.next().map(str::trim).filter(|path| !path.is_empty());
                        if let (Some(address), Some(path)) = (address, path) {
                            let token = auth.lock().map_err(|_| "authentication unavailable")?.begin_pairing(workspace.clone())?;
                            let invitation = host::HostInvitation { version: 1, address, server_name: "lapis-host".to_owned(), certificate: identity.certificate.as_ref().to_vec(),
                                workspace_id: workspace.as_str().to_owned(), name: options.workspace.file_name().unwrap_or_default().to_string_lossy().into_owned(), token: token.secret_bytes().to_vec() };
                            match host::write_invitation(std::path::Path::new(path), &invitation) {
                                Ok(()) => println!("招待を作成しました（1時間・一回限り）。安全な経路でClientへ渡してください。"),
                                Err(_) => eprintln!("招待ファイルを作成できません。新規ファイルを指定してください。"),
                            }
                        } else { eprintln!("Usage: invite <address:port> <new-file.json>"); }
                    } else if let Some(input) = line.strip_prefix("invite-oidc ") {
                        let mut parts = input.splitn(2, char::is_whitespace);
                        let address = parts.next().and_then(|value| value.parse().ok());
                        let path = parts.next().map(str::trim).filter(|path| !path.is_empty());
                        if let (Some(address), Some(path)) = (address, path) {
                            if !host::oidc_configured() {
                                eprintln!("OIDC設定がありません（LAPIS_OIDC_* を設定してください）");
                            } else {
                                let invitation = host::HostInvitation { version: 1, address, server_name: "lapis-host".to_owned(), certificate: identity.certificate.as_ref().to_vec(),
                                    workspace_id: workspace.as_str().to_owned(), name: options.workspace.file_name().unwrap_or_default().to_string_lossy().into_owned(), token: Vec::new() };
                                match host::write_invitation(std::path::Path::new(path), &invitation) {
                                    Ok(()) => println!("OIDC招待を作成しました。IdPアカウントでログインできます。"),
                                    Err(_) => eprintln!("招待ファイルを作成できません。新規ファイルを指定してください。"),
                                }
                            }
                        } else { eprintln!("Usage: invite-oidc <address:port> <new-file.json>"); }
                    } else if let Some(id) = line.strip_prefix("revoke ") {
                        match CredentialId::parse(id.to_owned()) {
                            Ok(id) => println!("revoked={}", auth.lock().map_err(|_| "authentication unavailable")?.revoke(&id)),
                            Err(_) => eprintln!("Invalid credential ID"),
                        }
                    } else if line == "devices" {
                        let auth = auth.lock().map_err(|_| "authentication unavailable")?;
                        let devices = auth.devices();
                        if devices.is_empty() {
                            println!("No devices");
                        } else {
                            for device in devices {
                                println!(
                                    "{} status={:?} client={} name={} kind={}",
                                    device.id,
                                    device.status,
                                    device.client_id,
                                    device.name,
                                    device.kind
                                );
                            }
                        }
                    } else if let Some(id) = line.strip_prefix("approve ") {
                        let id = id.trim();
                        if id.is_empty() {
                            eprintln!("Usage: approve <device-id>");
                        } else {
                            println!(
                                "approved={}",
                                auth.lock()
                                    .map_err(|_| "authentication unavailable")?
                                    .approve_device(id)
                            );
                        }
                    } else if let Some(id) = line.strip_prefix("revoke-device ") {
                        let id = id.trim();
                        if id.is_empty() {
                            eprintln!("Usage: revoke-device <device-id>");
                        } else {
                            println!(
                                "revoked={}",
                                auth.lock()
                                    .map_err(|_| "authentication unavailable")?
                                    .revoke_device(id)
                            );
                        }
                    } else if let Some(input) = line.strip_prefix("ext-install ") {
                        // `ext-install <file.lapis> [--disabled]`。読み込んだ宣言は
                        // `host.extensions.list` に現れ、有効な拡張は入口の再起動で反映する。
                        let mut parts = input.split_whitespace();
                        let path = parts.next();
                        let enabled = !parts.any(|argument| argument == "--disabled");
                        match path {
                            Some(path) => match host.install_extension(std::path::Path::new(path), enabled) {
                                Ok(outcome) => println!("{}", describe_outcome(&outcome)),
                                Err(error) => eprintln!("拡張を読み込めません: {error}"),
                            },
                            None => eprintln!("Usage: ext-install <file.lapis> [--disabled]"),
                        }
                    } else if let Some(input) = line.strip_prefix("ext-enable ") {
                        let id = input.trim();
                        if id.is_empty() {
                            eprintln!("Usage: ext-enable <extension-id>");
                        } else {
                            match host.set_extension_enabled(id, true) {
                                Ok(Some(registration)) => println!(
                                    "enabled={id} registration={registration:?}（入口の再起動で反映）"
                                ),
                                Ok(None) => eprintln!("有効にできない拡張です: {id}"),
                                Err(error) => eprintln!("拡張を有効にできません: {error}"),
                            }
                        }
                    } else if let Some(input) = line.strip_prefix("ext-disable ") {
                        let id = input.trim();
                        if id.is_empty() {
                            eprintln!("Usage: ext-disable <extension-id>");
                        } else {
                            match host.set_extension_enabled(id, false) {
                                Ok(Some(registration)) => println!(
                                    "disabled={id} registration={registration:?}（入口の再起動で反映）"
                                ),
                                Ok(None) => eprintln!("無効にできない拡張です: {id}"),
                                Err(error) => eprintln!("拡張を無効にできません: {error}"),
                            }
                        }
                    } else if let Some(input) = line.strip_prefix("ext-remove ") {
                        let id = input.trim();
                        if id.is_empty() {
                            eprintln!("Usage: ext-remove <extension-id>");
                        } else {
                            match host.remove_extension(id) {
                                Ok(()) => println!("removed={id}（拡張データは残します）"),
                                Err(error) => eprintln!("拡張を取り除けません: {error}"),
                            }
                        }
                    } else { eprintln!("Unknown command"); }
                    std::io::stdout().flush()?;
                }
            }
        }
        save_credentials(
            credential_store.as_mut(),
            &auth,
            &mut saved_credentials,
        )?;
        server.shutdown().await;
        if let Some(path) = &options.connection_info {
            remove_connection_info(path, address);
        }
        // 期限切れのtokenを残すと次回の接続先と誤認するため、正常終了で消す。
        if let Some(path) = &options.invitation {
            let _ = std::fs::remove_file(path);
        }
        Ok::<_, Error>(())
    })
}

fn save_credentials(
    store: Option<&mut CredentialStore>,
    auth: &Mutex<RemoteAuth>,
    last_saved: &mut Option<(Vec<u8>, Vec<u8>)>,
) -> Result<(), Error> {
    let Some(store) = store else {
        return Ok(());
    };
    let (mut credentials, mut devices) = {
        let auth = auth.lock().map_err(|_| "authentication unavailable")?;
        (auth.export_credentials(), auth.export_devices())
    };
    credentials.sort_by(|left, right| left.id.cmp(&right.id));
    devices.sort_by(|left, right| left.id.cmp(&right.id));
    let credential_bytes = serde_json::to_vec(&credentials)?;
    let device_bytes = serde_json::to_vec(&devices)?;
    if last_saved
        .as_ref()
        .is_some_and(|(saved_credentials, saved_devices)| {
            saved_credentials == &credential_bytes && saved_devices == &device_bytes
        })
    {
        return Ok(());
    }
    store.replace_auth(&credentials, &devices)?;
    *last_saved = Some((credential_bytes, device_bytes));
    Ok(())
}

fn auto_approve_oidc_devices() -> bool {
    std::env::var("LAPIS_AUTO_APPROVE_OIDC_DEVICES")
        .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

/// `ext-install` の結果を console 向けの 1 行にする。
fn describe_outcome(outcome: &extensions_entry::InstallOutcome) -> String {
    match outcome {
        extensions_entry::InstallOutcome::Installed {
            id,
            version,
            enabled,
        } => format!(
            "installed={id} version={version} enabled={enabled}（有効なら入口の再起動で反映）"
        ),
        extensions_entry::InstallOutcome::Incompatible {
            id,
            version,
            reason,
            kept_previous,
        } => {
            if *kept_previous {
                format!(
                    "incompatible={id} version={version}: {reason}（読み込まず、導入済みの版を残します）"
                )
            } else {
                format!("incompatible={id} version={version}: {reason}（読み込みません）")
            }
        }
    }
}

fn write_connection_info(
    path: &std::path::Path,
    connection: &wire::HostConnection,
) -> Result<(), Error> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    std::fs::create_dir_all(parent)?;
    wire::write_connection(path, connection)?;
    Ok(())
}

/// 自分が書いた接続情報だけを消す。後から起動したHostの情報を残す。
fn remove_connection_info(path: &std::path::Path, address: SocketAddr) {
    let owned = wire::read_connection(path).is_ok_and(|connection| connection.address == address);
    if !owned {
        return;
    }
    if let Err(error) = std::fs::remove_file(path) {
        eprintln!("接続情報を削除できません ({}): {error}", path.display());
    }
}

fn write_public_certificate(
    path: &std::path::Path,
    bytes: &[u8],
    replace: bool,
) -> Result<(), Error> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    if replace {
        temporary.persist(path)?;
    } else {
        temporary.persist_noclobber(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn public_certificate_is_replaced_only_when_requested() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("host.der");
        write_public_certificate(&path, b"first", false).unwrap();
        assert!(write_public_certificate(&path, b"second", false).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
        write_public_certificate(&path, b"second", true).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
    }
    #[test]
    fn requires_explicit_workspace_and_new_certificate_path() {
        assert!(Options::parse([].into_iter()).is_err());
        assert!(Options::parse(["--workspace", "repo"].map(Into::into).into_iter()).is_err());
        assert!(Options::parse(["--unknown", "value"].map(Into::into).into_iter()).is_err());
        let options = Options::parse(
            ["--workspace", "repo", "--certificate", "host.der"]
                .map(Into::into)
                .into_iter(),
        )
        .unwrap();
        assert!(options.listen.ip().is_loopback());
        assert_eq!(options.workspace, PathBuf::from("repo"));
        assert!(options.state_directory.is_none());
        assert!(options.connection_info.is_none());
        let options = Options::parse(
            [
                "--workspace",
                "repo",
                "--certificate",
                "host.der",
                "--state-directory",
                "state",
                "--connection-info",
                "dev/connection.json",
            ]
            .map(Into::into)
            .into_iter(),
        )
        .unwrap();
        assert_eq!(options.state_directory, Some(PathBuf::from("state")));
        assert_eq!(
            options.connection_info,
            Some(PathBuf::from("dev/connection.json"))
        );
    }

    #[test]
    fn connection_info_is_only_removed_by_its_owner() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("dev").join("connection.json");
        let connection = wire::HostConnection {
            version: wire::HOST_CONNECTION_VERSION,
            address: "127.0.0.1:1".parse().unwrap(),
            server_name: "lapis-host".to_owned(),
            workspace_id: "workspace-1".to_owned(),
            certificate: vec![1, 2, 3],
        };
        write_connection_info(&path, &connection).unwrap();
        assert!(path.is_file());

        // 後から起動したHostの接続情報は、先のHostの終了では消さない。
        let other = wire::HostConnection {
            address: "127.0.0.1:2".parse().unwrap(),
            ..connection.clone()
        };
        write_connection_info(&path, &other).unwrap();
        remove_connection_info(&path, connection.address);
        assert!(path.is_file());

        remove_connection_info(&path, other.address);
        assert!(!path.exists());
    }
}
