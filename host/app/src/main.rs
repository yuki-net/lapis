use host::BackendHost;
use platform::CredentialStore;
use remote::{
    AuthConfig, AuthPolicy, CredentialId, CredentialLifetime, PairingLifetime, QuicServer,
    RemoteAuth, RemoteLimits, tls13_server_config,
};
use rustls::pki_types::PrivateKeyDer;
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
    replace_certificate: bool,
}

impl Options {
    fn parse(arguments: impl Iterator<Item = std::ffi::OsString>) -> Result<Self, Error> {
        let mut arguments = arguments;
        let mut workspace = None;
        let mut certificate = None;
        let mut listen = "127.0.0.1:0".parse()?;
        let mut state_directory = None;
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
                _ => return Err("unknown option".into()),
            }
        }
        Ok(Self {
            workspace: workspace.ok_or("--workspace is required")?,
            certificate: certificate.ok_or("--certificate is required")?,
            listen,
            state_directory,
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
            "Usage: lapis-host --workspace <directory> --certificate <public.der> [--replace-certificate] [--listen <ip:port>] [--state-directory <directory>]"
        );
        std::process::exit(1);
    }
}

fn run() -> Result<(), Error> {
    let options = Options::parse(std::env::args_os().skip(1))?;
    let state_directory = options.state_directory.clone();
    let host = match options.state_directory {
        Some(directory) => BackendHost::start_with_state_directory(directory)?,
        None => BackendHost::start()?,
    };
    let credentials_path = state_directory.map(|directory| directory.join(CREDENTIAL_DB));
    let workspace = host.register_workspace(&options.workspace)?;
    // 秘密鍵はHostのメモリ内だけに保持する。
    let certified =
        rcgen::generate_simple_self_signed(vec!["localhost".to_owned(), "lapis-host".to_owned()])?;
    let key = Zeroizing::new(certified.signing_key.serialize_der());
    let tls = tls13_server_config(
        vec![certified.cert.der().clone()],
        PrivateKeyDer::Pkcs8(key.to_vec().into()),
    )?;
    let auth = Arc::new(Mutex::new(RemoteAuth::system(
        AuthConfig::new(
            PairingLifetime::new(3600)?,
            CredentialLifetime::new(CREDENTIAL_LIFETIME_SECONDS)?,
        ),
        AuthPolicy::new(host::supported_capabilities()),
    )));
    if let Some(oidc) = host::oidc_from_env()? {
        auth.lock()
            .map_err(|_| "authentication unavailable")?
            .enable_oidc(oidc.verifier, oidc.client);
    }
    let mut credential_store = match &credentials_path {
        Some(path) => match CredentialStore::open(path) {
            Ok(store) => match store.load() {
                Ok(records) => {
                    let loaded = {
                        let mut auth = auth.lock().map_err(|_| "authentication unavailable")?;
                        auth.import_credentials(records);
                        auth.export_credentials().len()
                    };
                    println!("Loaded {loaded} credential(s) from {}", path.display());
                    Some(store)
                }
                Err(error) => {
                    eprintln!("Ignoring invalid credential database: {error}");
                    None
                }
            },
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
        write_public_certificate(&options.certificate, certified.cert.der(), options.replace_certificate)?;
        println!("address={} server_name=lapis-host workspace={}", server.listening_address()?, workspace.as_str());
        println!("Commands: pair / pair-user / register <directory> / invite <address:port> <new-file.json> / revoke <credential-id> / quit");
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
                            let invitation = host::HostInvitation { version: 1, address, server_name: "lapis-host".to_owned(), certificate: certified.cert.der().to_vec(),
                                workspace_id: workspace.as_str().to_owned(), name: options.workspace.file_name().unwrap_or_default().to_string_lossy().into_owned(), token: token.secret_bytes().to_vec() };
                            match host::write_invitation(std::path::Path::new(path), &invitation) {
                                Ok(()) => println!("招待を作成しました（1時間・一回限り）。安全な経路でClientへ渡してください。"),
                                Err(_) => eprintln!("招待ファイルを作成できません。新規ファイルを指定してください。"),
                            }
                        } else { eprintln!("Usage: invite <address:port> <new-file.json>"); }
                    } else if let Some(id) = line.strip_prefix("revoke ") {
                        match CredentialId::parse(id.to_owned()) {
                            Ok(id) => println!("revoked={}", auth.lock().map_err(|_| "authentication unavailable")?.revoke(&id)),
                            Err(_) => eprintln!("Invalid credential ID"),
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
        Ok::<_, Error>(())
    })
}

fn save_credentials(
    store: Option<&mut CredentialStore>,
    auth: &Mutex<RemoteAuth>,
    last_saved: &mut Option<Vec<u8>>,
) -> Result<(), Error> {
    let Some(store) = store else {
        return Ok(());
    };
    let mut records = auth
        .lock()
        .map_err(|_| "authentication unavailable")?
        .export_credentials();
    records.sort_by(|left, right| left.id.cmp(&right.id));
    let bytes = serde_json::to_vec(&records)?;
    if last_saved.as_deref() == Some(bytes.as_slice()) {
        return Ok(());
    }
    store.replace_all(&records)?;
    *last_saved = Some(bytes);
    Ok(())
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
        let options = Options::parse(
            [
                "--workspace",
                "repo",
                "--certificate",
                "host.der",
                "--state-directory",
                "state",
            ]
            .map(Into::into)
            .into_iter(),
        )
        .unwrap();
        assert_eq!(options.state_directory, Some(PathBuf::from("state")));
    }
}
