use lapis_host::BackendHost;
use lapis_remote::{
    AuthConfig, AuthPolicy, BackendRemoteHandler, CredentialId, CredentialLifetime,
    PairingLifetime, QuicServer, RemoteAuth, RemoteLimits, tls13_server_config,
};
use rustls::pki_types::PrivateKeyDer;
use std::{
    io::{BufRead, Write},
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use zeroize::Zeroizing;

type Error = Box<dyn std::error::Error + Send + Sync>;

struct Options {
    workspace: PathBuf,
    certificate: PathBuf,
    listen: SocketAddr,
}

impl Options {
    fn parse(arguments: impl Iterator<Item = std::ffi::OsString>) -> Result<Self, Error> {
        let mut arguments = arguments;
        let mut workspace = None;
        let mut certificate = None;
        let mut listen = "127.0.0.1:0".parse()?;
        while let Some(argument) = arguments.next() {
            let value = arguments.next().ok_or("option requires a value")?;
            match argument.to_str() {
                Some("--workspace") => workspace = Some(PathBuf::from(value)),
                Some("--certificate") => certificate = Some(PathBuf::from(value)),
                Some("--listen") => listen = value.to_str().ok_or("invalid address")?.parse()?,
                _ => return Err("unknown option".into()),
            }
        }
        Ok(Self {
            workspace: workspace.ok_or("--workspace is required")?,
            certificate: certificate.ok_or("--certificate is required")?,
            listen,
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
            .and_then(|root| lapis_host::run_managed_host(std::path::Path::new(root)));
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
                lapis_platform::run_task_worker(std::path::Path::new(path)).map_err(Into::into)
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
            "Usage: lapis-host --workspace <directory> --certificate <new-public.der> [--listen <ip:port>]"
        );
        std::process::exit(1);
    }
}

fn run() -> Result<(), Error> {
    let options = Options::parse(std::env::args_os().skip(1))?;
    let host = BackendHost::start()?;
    let workspace = host.register_workspace(&options.workspace)?;
    // 秘密鍵はHostのメモリ内だけに保持する。公開証明書は明示した新規ファイルへ出力。
    let certified =
        rcgen::generate_simple_self_signed(vec!["localhost".to_owned(), "lapis-host".to_owned()])?;
    let key = Zeroizing::new(certified.signing_key.serialize_der());
    let tls = tls13_server_config(
        vec![certified.cert.der().clone()],
        PrivateKeyDer::Pkcs8(key.to_vec().into()),
    )?;
    let mut certificate = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&options.certificate)?;
    certificate.write_all(certified.cert.der())?;
    certificate.sync_all()?;
    let auth = Arc::new(Mutex::new(RemoteAuth::system(
        AuthConfig::new(PairingLifetime::new(300)?, CredentialLifetime::new(86400)?),
        AuthPolicy::new(lapis_host::supported_capabilities()),
    )));
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
            Arc::new(BackendRemoteHandler::new(host.service())), RemoteLimits::default()).await?;
        println!("address={} server_name=lapis-host workspace={}", server.listening_address()?, workspace.as_str());
        println!("Commands: pair / invite <address:port> <new-file.json> / revoke <credential-id> / quit");
        std::io::stdout().flush()?;
        let mut input_open = true;
        loop {
            tokio::select! {
                signal = tokio::signal::ctrl_c() => { signal?; break; }
                line = input.recv(), if input_open => {
                    let Some(line) = line else { input_open = false; continue };
                    let line = line?;
                    let line = line.trim();
                    if line == "quit" { break; }
                    if line == "pair" {
                        let token = auth.lock().map_err(|_| "authentication unavailable")?.begin_pairing(workspace.clone())?;
                        // 利用者が要求したローカル招待だけを表示し、ログへ自動出力しない。
                        let mut invitation = Zeroizing::new(String::with_capacity(64));
                        for byte in token.secret_bytes() {
                            use std::fmt::Write;
                            write!(invitation, "{byte:02x}")?;
                        }
                        println!("Pairing token (5 minutes, one use): {}", invitation.as_str());
                    } else if let Some(input) = line.strip_prefix("invite ") {
                        let mut parts = input.splitn(2, char::is_whitespace);
                        let address = parts.next().and_then(|value| value.parse().ok());
                        let path = parts.next().map(str::trim).filter(|path| !path.is_empty());
                        if let (Some(address), Some(path)) = (address, path) {
                            let token = auth.lock().map_err(|_| "authentication unavailable")?.begin_pairing(workspace.clone())?;
                            let invitation = lapis_host::HostInvitation { version: 1, address, server_name: "lapis-host".to_owned(), certificate: certified.cert.der().to_vec(),
                                workspace_id: workspace.as_str().to_owned(), name: options.workspace.file_name().unwrap_or_default().to_string_lossy().into_owned(), token: token.secret_bytes().to_vec() };
                            match lapis_host::write_invitation(std::path::Path::new(path), &invitation) {
                                Ok(()) => println!("招待を作成しました（5分間・一回限り）。安全な経路でClientへ渡してください。"),
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
        server.shutdown().await;
        Ok::<_, Error>(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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
    }
}
