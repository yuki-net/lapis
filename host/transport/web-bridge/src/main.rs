use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use axum::{
    Router,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use quinn::crypto::rustls::QuicClientConfig;
use rustls::pki_types::CertificateDer;
use tokio::sync::Semaphore;
use tokio_util::codec::LengthDelimitedCodec;

mod oidc;

const MAX_MESSAGE: usize = 16 * 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(30);
/// 接続情報を待つ間、待機中であることを知らせる間隔。
const CONNECTION_LOG_INTERVAL: Duration = Duration::from_secs(10);
const CONNECTION_POLL: Duration = Duration::from_millis(100);
/// 開発時に `.run` から引数を消せるよう、待受とOriginに既定値を持たせる。
const DEFAULT_LISTEN: &str = "127.0.0.1:4757";
const DEFAULT_ORIGIN: &str = "http://127.0.0.1:4747";
const LISTEN_ENV: &str = "LAPIS_WEB_BRIDGE_LISTEN";
const ORIGIN_ENV: &str = "LAPIS_WEB_BRIDGE_ORIGIN";
type Error = Box<dyn std::error::Error + Send + Sync>;

struct Options {
    listen: SocketAddr,
    target: Target,
    origin: String,
}

/// Hostへの接続先の決め方。
#[derive(Clone)]
enum Target {
    /// 既知のアドレス・サーバー名・公開証明書を使う。
    Explicit {
        host: SocketAddr,
        server_name: String,
        certificate: PathBuf,
    },
    /// Hostが書き出した接続情報を接続のたびに読み直す。
    Connection { path: PathBuf },
}

struct Resolved {
    host: SocketAddr,
    server_name: String,
    certificate: Vec<u8>,
    /// 接続情報から読んだ場合だけ分かる。
    workspace_id: Option<String>,
}

impl Target {
    fn resolve(&self) -> Result<Resolved, String> {
        match self {
            Self::Explicit {
                host,
                server_name,
                certificate,
            } => Ok(Resolved {
                host: *host,
                server_name: server_name.clone(),
                certificate: std::fs::read(certificate).map_err(|error| {
                    format!(
                        "証明書を読み込めません ({}): {error}",
                        certificate.display()
                    )
                })?,
                workspace_id: None,
            }),
            Self::Connection { path } => {
                let connection = wire::read_connection(path)?;
                if !connection.address.ip().is_loopback() {
                    return Err(format!(
                        "接続情報のHostがloopbackではありません ({})",
                        path.display()
                    ));
                }
                Ok(Resolved {
                    host: connection.address,
                    server_name: connection.server_name,
                    certificate: connection.certificate,
                    workspace_id: Some(connection.workspace_id),
                })
            }
        }
    }
}

impl Options {
    fn parse(args: impl Iterator<Item = std::ffi::OsString>) -> Result<Self, Error> {
        Self::parse_with(args, |name| std::env::var(name).ok())
    }

    /// 優先順はCLI引数、環境変数、既定値。テストから環境変数を差し替えるため分離する。
    fn parse_with(
        args: impl Iterator<Item = std::ffi::OsString>,
        setting: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, Error> {
        let mut args = args;
        let (mut listen, mut host, mut server_name, mut certificate, mut origin) =
            (None, None, None, None, None);
        let mut connection_info = None;
        let mut allow_remote = false;
        while let Some(flag) = args.next() {
            if flag.to_str() == Some("--allow-remote") {
                allow_remote = true;
                continue;
            }
            let value = args.next().ok_or("option requires a value")?;
            match flag.to_str() {
                Some("--listen") => {
                    listen = Some(value.into_string().map_err(|_| "invalid listen")?)
                }
                Some("--host") => {
                    host = Some(
                        value
                            .to_str()
                            .ok_or("invalid host")?
                            .parse::<SocketAddr>()?,
                    )
                }
                Some("--server-name") => {
                    server_name = Some(value.into_string().map_err(|_| "invalid server name")?)
                }
                Some("--certificate") => certificate = Some(PathBuf::from(value)),
                Some("--connection-info") => connection_info = Some(PathBuf::from(value)),
                Some("--origin") => {
                    origin = Some(value.into_string().map_err(|_| "invalid origin")?)
                }
                _ => return Err("unknown option".into()),
            }
        }
        let listen = listen
            .or_else(|| setting(LISTEN_ENV))
            .unwrap_or_else(|| DEFAULT_LISTEN.to_owned())
            .parse::<SocketAddr>()
            .map_err(|error| format!("invalid listen address: {error}"))?;
        if !listen.ip().is_loopback() && !allow_remote {
            return Err("non-loopback listen requires --allow-remote".into());
        }
        let target = match connection_info {
            Some(path) => {
                if host.is_some() || server_name.is_some() || certificate.is_some() {
                    return Err("--connection-info cannot be combined with --host, --server-name, or --certificate".into());
                }
                Target::Connection { path }
            }
            None => {
                let host = host.ok_or("--host is required")?;
                if !host.ip().is_loopback() {
                    return Err("host must use a loopback address".into());
                }
                Target::Explicit {
                    host,
                    server_name: server_name.ok_or("--server-name is required")?,
                    certificate: certificate.ok_or("--certificate is required")?,
                }
            }
        };
        let origin = origin
            .or_else(|| setting(ORIGIN_ENV))
            .unwrap_or_else(|| DEFAULT_ORIGIN.to_owned());
        let authority = origin
            .strip_prefix("http://")
            .or_else(|| origin.strip_prefix("https://"))
            .ok_or("origin must be an http(s) origin")?;
        if authority.is_empty()
            || authority.contains('/')
            || authority.contains('?')
            || authority.contains('#')
            || authority.chars().any(char::is_whitespace)
        {
            return Err("origin must be an http(s) origin without a path".into());
        }
        Ok(Self {
            listen,
            target,
            origin,
        })
    }
}

#[derive(Clone)]
struct Bridge {
    endpoint: quinn::Endpoint,
    target: Target,
    origin: String,
    slots: Arc<Semaphore>,
    oidc: Option<Arc<oidc::OidcBroker>>,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let options = Options::parse(std::env::args_os().skip(1))?;
    let oidc = oidc::OidcBroker::from_env()?;
    if oidc.is_some() {
        eprintln!("web-bridge OIDC login enabled");
    }
    let resolved = match &options.target {
        Target::Connection { path } => wait_for_connection(path).await?,
        Target::Explicit { .. } => options.target.resolve()?,
    };
    let Resolved {
        host,
        certificate,
        workspace_id,
        ..
    } = resolved;
    let endpoint = quinn::Endpoint::client(
        if host.is_ipv4() {
            "127.0.0.1:0"
        } else {
            "[::1]:0"
        }
        .parse()?,
    )?;
    // 起動時に証明書を検証しておく。壊れていればここで止まる。
    let _ = client_config(certificate)?;
    let listener = tokio::net::TcpListener::bind(options.listen).await?;
    let listen = listener.local_addr()?;
    let open = open_url(&options.origin);
    let bridge = Bridge {
        endpoint,
        target: options.target,
        origin: options.origin,
        slots: Arc::new(Semaphore::new(128)),
        oidc: oidc.map(Arc::new),
    };
    eprintln!("web-bridge listening on {listen}");
    if let Some(workspace) = workspace_id.as_deref() {
        eprintln!("web-bridge workspace {workspace}");
    }
    eprintln!("open {open}");
    axum::serve(
        listener,
        Router::new()
            .route(wire::REMOTE_WEBSOCKET_PATH, get(upgrade))
            .route("/auth/oidc/start", get(oidc::start))
            .route("/auth/oidc/callback", get(oidc::callback))
            .with_state(bridge),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    Ok(())
}

fn client_config(certificate: Vec<u8>) -> Result<quinn::ClientConfig, Error> {
    let certificate = CertificateDer::from(certificate);
    let mut roots = rustls::RootCertStore::empty();
    roots.add(certificate)?;
    let mut tls = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])?
    .with_root_certificates(roots)
    .with_no_client_auth();
    tls.alpn_protocols = vec![wire::REMOTE_QUIC_PROTOCOL.to_vec()];
    tls.enable_early_data = false;
    Ok(quinn::ClientConfig::new(Arc::new(
        QuicClientConfig::try_from(tls)?,
    )))
}

/// Hostが接続情報を書き出すまで待つ。Hostより先に起動しても繋がるようにする。
/// 上限を設けるとHostの初回ビルドに間に合わず起動できなくなるため、読めるまで待ち続ける。
async fn wait_for_connection(path: &std::path::Path) -> Result<Resolved, Error> {
    let target = Target::Connection {
        path: path.to_path_buf(),
    };
    let mut next_log = tokio::time::Instant::now();
    loop {
        match target.resolve() {
            Ok(resolved) => return Ok(resolved),
            Err(error) => {
                let now = tokio::time::Instant::now();
                if now >= next_log {
                    eprintln!("web-bridge waiting for connection info ({error})");
                    next_log = now + CONNECTION_LOG_INTERVAL;
                }
                tokio::time::sleep(CONNECTION_POLL).await;
            }
        }
    }
}

/// ブラウザーで開くURL。接続先や token は URL に載せない。
fn open_url(origin: &str) -> String {
    format!("{}/", origin.trim_end_matches('/'))
}

async fn upgrade(
    State(bridge): State<Bridge>,
    headers: HeaderMap,
    socket: WebSocketUpgrade,
) -> Response {
    if headers
        .get(axum::http::header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        != Some(bridge.origin.as_str())
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    let socket = socket.protocols([wire::REMOTE_WEBSOCKET_PROTOCOL]);
    if socket.selected_protocol().is_none() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Ok(permit) = bridge.slots.clone().try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    socket
        .max_frame_size(MAX_MESSAGE)
        .max_message_size(MAX_MESSAGE)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            if let Err(error) = forward(socket, bridge).await {
                eprintln!("web-bridge connection ended: {error}");
            }
        })
}

async fn forward(socket: WebSocket, bridge: Bridge) -> Result<(), Error> {
    // 接続のたびに読み直し、Hostの再起動でアドレスや証明書が変わっても追従する。
    let Resolved {
        host,
        server_name,
        certificate,
        ..
    } = bridge.target.resolve()?;
    let config = client_config(certificate)?;
    let connection = tokio::time::timeout(
        Duration::from_secs(10),
        bridge.endpoint.connect_with(config, host, &server_name)?,
    )
    .await??;
    let (send, recv) =
        tokio::time::timeout(Duration::from_secs(10), connection.open_bi()).await??;
    let mut codec = LengthDelimitedCodec::builder();
    codec.max_frame_length(MAX_MESSAGE);
    let mut reader = codec.new_read(recv);
    let mut writer = codec.new_write(send);
    let (mut ws_send, mut ws_recv) = socket.split();
    loop {
        tokio::select! {
            incoming = ws_recv.next() => match incoming {
                Some(Ok(Message::Text(text))) => tokio::time::timeout(IO_TIMEOUT, writer.send(text.as_bytes().to_vec().into())).await??,
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {},
                Some(Ok(Message::Close(_))) | None => break,
                _ => return Err("unsupported WebSocket message".into()),
            },
            incoming = reader.next() => match incoming {
                Some(Ok(bytes)) => {
                    let text = String::from_utf8(bytes.to_vec())?;
                    tokio::time::timeout(IO_TIMEOUT, ws_send.send(Message::Text(text.into()))).await??;
                }
                Some(Err(error)) => return Err(error.into()),
                None => break,
            }
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(1), writer.close()).await;
    connection.close(0u32.into(), b"client disconnected");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::tungstenite::{Message as ClientMessage, client::IntoClientRequest};

    #[test]
    fn options_require_explicit_remote_listen_and_reject_bad_origins() {
        let base = [
            "--host",
            "127.0.0.1:5000",
            "--server-name",
            "localhost",
            "--certificate",
            "cert.der",
        ];
        let parse = |extra: &[&str]| {
            let args = base
                .iter()
                .chain(extra.iter())
                .map(|value| std::ffi::OsString::from(*value))
                .collect::<Vec<_>>();
            Options::parse(args.into_iter())
        };

        // 既定ではループバック以外で待ち受けない。
        assert!(
            parse(&[
                "--listen",
                "0.0.0.0:8080",
                "--origin",
                "http://localhost:5173"
            ])
            .is_err()
        );
        // 明示した場合だけ、指定したoriginでTailscaleなどから待ち受ける。
        let remote = parse(&[
            "--listen",
            "0.0.0.0:8080",
            "--allow-remote",
            "--origin",
            "http://100.64.0.1:5174",
        ])
        .unwrap();
        assert!(remote.listen.ip().is_unspecified());
        // Hostへの接続は常にループバック。
        assert!(
            parse(&[
                "--listen",
                "127.0.0.1:8080",
                "--host",
                "192.0.2.1:5000",
                "--origin",
                "http://localhost:5173",
            ])
            .is_err()
        );
        // originにパスは含めない。
        assert!(
            parse(&[
                "--listen",
                "127.0.0.1:8080",
                "--origin",
                "https://example.test/lapis",
            ])
            .is_err()
        );
        // 接続情報と明示指定は併用できない。
        assert!(
            parse(&[
                "--listen",
                "127.0.0.1:8080",
                "--origin",
                "http://localhost:5173",
                "--connection-info",
                "connection.json",
            ])
            .is_err()
        );
        // 接続情報だけを指定すればHostの値は不要。
        let connection = Options::parse(
            [
                "--listen",
                "127.0.0.1:8080",
                "--origin",
                "http://localhost:5173",
                "--connection-info",
                "connection.json",
            ]
            .map(std::ffi::OsString::from)
            .into_iter(),
        )
        .unwrap();
        assert!(matches!(connection.target, Target::Connection { .. }));
    }

    #[test]
    fn defaults_and_environment_order_decide_listen_and_origin() {
        let parse = |args: &[&str], environment: &[(&str, &str)]| {
            Options::parse_with(
                args.iter().map(|value| std::ffi::OsString::from(*value)),
                |name| {
                    environment
                        .iter()
                        .find(|(key, _)| *key == name)
                        .map(|(_, value)| (*value).to_owned())
                },
            )
        };
        let host = [
            "--host",
            "127.0.0.1:5000",
            "--server-name",
            "localhost",
            "--certificate",
            "cert.der",
        ];
        // 引数を省いても既定値で起動できる。
        let defaults = parse(&host, &[]).unwrap();
        assert_eq!(defaults.listen, "127.0.0.1:4757".parse().unwrap());
        assert_eq!(defaults.origin, "http://127.0.0.1:4747");
        // 環境変数は既定値より優先する。
        let environment = parse(
            &host,
            &[
                (LISTEN_ENV, "127.0.0.1:5001"),
                (ORIGIN_ENV, "http://127.0.0.1:5002"),
            ],
        )
        .unwrap();
        assert_eq!(environment.listen, "127.0.0.1:5001".parse().unwrap());
        assert_eq!(environment.origin, "http://127.0.0.1:5002");
        // CLI引数は環境変数より優先する。
        let cli = parse(
            &[
                "--listen",
                "127.0.0.1:5003",
                "--origin",
                "http://127.0.0.1:5004",
                "--host",
                "127.0.0.1:5000",
                "--server-name",
                "localhost",
                "--certificate",
                "cert.der",
            ],
            &[
                (LISTEN_ENV, "127.0.0.1:5001"),
                (ORIGIN_ENV, "http://127.0.0.1:5002"),
            ],
        )
        .unwrap();
        assert_eq!(cli.listen, "127.0.0.1:5003".parse().unwrap());
        assert_eq!(cli.origin, "http://127.0.0.1:5004");
        // 環境変数でもループバック以外は --allow-remote が要る。
        assert!(parse(&host, &[(LISTEN_ENV, "0.0.0.0:5001")]).is_err());
        // 環境変数の不正値は既定へ落とさずエラーにする。
        assert!(parse(&host, &[(ORIGIN_ENV, "localhost:4747")]).is_err());
        assert!(parse(&host, &[(LISTEN_ENV, "not-an-address")]).is_err());
    }

    #[test]
    fn open_url_does_not_include_connection_information() {
        assert_eq!(open_url("http://127.0.0.1:4747"), "http://127.0.0.1:4747/");
        assert_eq!(open_url("http://127.0.0.1:4747/"), "http://127.0.0.1:4747/");
    }

    #[tokio::test]
    async fn waits_for_connection_info_to_appear() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("connection.json");
        let writer = tokio::spawn({
            let path = path.clone();
            async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                wire::write_connection(
                    &path,
                    &wire::HostConnection {
                        version: wire::HOST_CONNECTION_VERSION,
                        address: "127.0.0.1:5000".parse().unwrap(),
                        server_name: "lapis-host".into(),
                        workspace_id: "workspace-1".into(),
                        certificate: vec![1, 2, 3],
                    },
                )
                .unwrap();
            }
        });

        let resolved = wait_for_connection(&path).await.unwrap();
        assert_eq!(resolved.host, "127.0.0.1:5000".parse().unwrap());
        assert_eq!(resolved.server_name, "lapis-host");
        assert_eq!(resolved.workspace_id.as_deref(), Some("workspace-1"));
        writer.await.unwrap();
    }

    #[tokio::test]
    async fn forwards_messages_and_rejects_wrong_origin() {
        let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        let certificate = certified.cert.der().clone();
        let key =
            rustls::pki_types::PrivateKeyDer::Pkcs8(certified.signing_key.serialize_der().into());
        let mut server_tls = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![certificate.clone()], key)
        .unwrap();
        server_tls.alpn_protocols = vec![wire::REMOTE_QUIC_PROTOCOL.to_vec()];
        let server_config = quinn::ServerConfig::with_crypto(Arc::new(
            quinn::crypto::rustls::QuicServerConfig::try_from(server_tls).unwrap(),
        ));
        let next = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        let next_certificate = next.cert.der().clone();
        let next_key =
            rustls::pki_types::PrivateKeyDer::Pkcs8(next.signing_key.serialize_der().into());
        let mut next_tls = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![next_certificate.clone()], next_key)
        .unwrap();
        next_tls.alpn_protocols = vec![wire::REMOTE_QUIC_PROTOCOL.to_vec()];
        let next_server = quinn::ServerConfig::with_crypto(Arc::new(
            quinn::crypto::rustls::QuicServerConfig::try_from(next_tls).unwrap(),
        ));
        let host = quinn::Endpoint::server(server_config, "127.0.0.1:0".parse().unwrap()).unwrap();
        let host_address = host.local_addr().unwrap();
        let (rotate, rotated) = tokio::sync::oneshot::channel();
        let (ready, wait_ready) = tokio::sync::oneshot::channel();
        let host_task = tokio::spawn(async move {
            let mut rotated = Some(rotated);
            let mut ready = Some(ready);
            let mut next_server = Some(next_server);
            for index in 0..2 {
                if index == 1 {
                    rotated.take().unwrap().await.unwrap();
                    host.set_server_config(next_server.take());
                    ready.take().unwrap().send(()).unwrap();
                }
                let connection = host.accept().await.unwrap().await.unwrap();
                let (send, recv) = connection.accept_bi().await.unwrap();
                let mut reader = LengthDelimitedCodec::builder().new_read(recv);
                let mut writer = LengthDelimitedCodec::builder().new_write(send);
                let message = reader.next().await.unwrap().unwrap();
                writer.send(message.freeze()).await.unwrap();
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
        let directory = tempfile::tempdir().unwrap();
        let certificate_path = directory.path().join("host.der");
        std::fs::write(&certificate_path, &certificate).unwrap();
        let mut roots = rustls::RootCertStore::empty();
        roots.add(certificate).unwrap();
        let mut tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
        tls.alpn_protocols = vec![wire::REMOTE_QUIC_PROTOCOL.to_vec()];
        let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        endpoint.set_default_client_config(quinn::ClientConfig::new(Arc::new(
            QuicClientConfig::try_from(tls).unwrap(),
        )));
        let bridge = Bridge {
            endpoint,
            target: Target::Explicit {
                host: host_address,
                server_name: "localhost".into(),
                certificate: certificate_path.clone(),
            },
            origin: "http://localhost:5173".into(),
            slots: Arc::new(Semaphore::new(2)),
            oidc: None,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .route(wire::REMOTE_WEBSOCKET_PATH, get(upgrade))
                    .with_state(bridge),
            )
            .await
            .unwrap();
        });
        let url = format!("ws://{address}/remote");
        let mut bad = url.as_str().into_client_request().unwrap();
        bad.headers_mut()
            .insert("origin", "http://attacker.example".parse().unwrap());
        bad.headers_mut().insert(
            "sec-websocket-protocol",
            wire::REMOTE_WEBSOCKET_PROTOCOL.parse().unwrap(),
        );
        assert!(tokio_tungstenite::connect_async(bad).await.is_err());
        let mut good = url.as_str().into_client_request().unwrap();
        good.headers_mut()
            .insert("origin", "http://localhost:5173".parse().unwrap());
        good.headers_mut().insert(
            "sec-websocket-protocol",
            wire::REMOTE_WEBSOCKET_PROTOCOL.parse().unwrap(),
        );
        let (mut socket, _) = tokio_tungstenite::connect_async(good).await.unwrap();
        socket
            .send(ClientMessage::Text("{\"type\":\"opaque\"}".into()))
            .await
            .unwrap();
        let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(message.into_text().unwrap(), "{\"type\":\"opaque\"}");
        drop(socket);
        std::fs::write(&certificate_path, next_certificate).unwrap();
        rotate.send(()).unwrap();
        wait_ready.await.unwrap();
        let mut good = url.as_str().into_client_request().unwrap();
        good.headers_mut()
            .insert("origin", "http://localhost:5173".parse().unwrap());
        good.headers_mut().insert(
            "sec-websocket-protocol",
            wire::REMOTE_WEBSOCKET_PROTOCOL.parse().unwrap(),
        );
        let (mut socket, _) = tokio_tungstenite::connect_async(good).await.unwrap();
        socket
            .send(ClientMessage::Text("second".into()))
            .await
            .unwrap();
        let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(message.into_text().unwrap(), "second");
        drop(socket);
        host_task.await.unwrap();
        task.abort();
    }

    #[tokio::test]
    async fn forwards_messages_using_connection_info() {
        let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        let certificate = certified.cert.der().clone();
        let key =
            rustls::pki_types::PrivateKeyDer::Pkcs8(certified.signing_key.serialize_der().into());
        let mut server_tls = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![certificate.clone()], key)
        .unwrap();
        server_tls.alpn_protocols = vec![wire::REMOTE_QUIC_PROTOCOL.to_vec()];
        let server_config = quinn::ServerConfig::with_crypto(Arc::new(
            quinn::crypto::rustls::QuicServerConfig::try_from(server_tls).unwrap(),
        ));
        let host = quinn::Endpoint::server(server_config, "127.0.0.1:0".parse().unwrap()).unwrap();
        let host_address = host.local_addr().unwrap();
        let host_task = tokio::spawn(async move {
            let connection = host.accept().await.unwrap().await.unwrap();
            let (send, recv) = connection.accept_bi().await.unwrap();
            let mut reader = LengthDelimitedCodec::builder().new_read(recv);
            let mut writer = LengthDelimitedCodec::builder().new_write(send);
            let message = reader.next().await.unwrap().unwrap();
            writer.send(message.freeze()).await.unwrap();
            // ブリッジが閉じるまで接続を維持し、エコーが届く前にQUICを閉じない。
            let _ = tokio::time::timeout(Duration::from_secs(10), connection.closed()).await;
        });
        let directory = tempfile::tempdir().unwrap();
        let connection_info = directory.path().join("connection.json");
        wire::write_connection(
            &connection_info,
            &wire::HostConnection {
                version: wire::HOST_CONNECTION_VERSION,
                address: host_address,
                server_name: "localhost".into(),
                workspace_id: "workspace-1".into(),
                certificate: certificate.to_vec(),
            },
        )
        .unwrap();
        let bridge = Bridge {
            endpoint: quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap(),
            target: Target::Connection {
                path: connection_info,
            },
            origin: "http://localhost:5173".into(),
            slots: Arc::new(Semaphore::new(1)),
            oidc: None,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .route(wire::REMOTE_WEBSOCKET_PATH, get(upgrade))
                    .with_state(bridge),
            )
            .await
            .unwrap();
        });
        let url = format!("ws://{address}/remote");
        let mut request = url.as_str().into_client_request().unwrap();
        request
            .headers_mut()
            .insert("origin", "http://localhost:5173".parse().unwrap());
        request.headers_mut().insert(
            "sec-websocket-protocol",
            wire::REMOTE_WEBSOCKET_PROTOCOL.parse().unwrap(),
        );
        let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
        socket
            .send(ClientMessage::Text("hello".into()))
            .await
            .unwrap();
        let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(message.into_text().unwrap(), "hello");
        drop(socket);
        host_task.await.unwrap();
        task.abort();
    }
}
