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

const MAX_MESSAGE: usize = 16 * 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(30);
type Error = Box<dyn std::error::Error + Send + Sync>;

struct Options {
    listen: SocketAddr,
    host: SocketAddr,
    server_name: String,
    certificate: PathBuf,
    origin: String,
}

impl Options {
    fn parse(args: impl Iterator<Item = std::ffi::OsString>) -> Result<Self, Error> {
        let mut args = args;
        let (mut listen, mut host, mut server_name, mut certificate, mut origin) =
            (None, None, None, None, None);
        while let Some(flag) = args.next() {
            let value = args.next().ok_or("option requires a value")?;
            match flag.to_str() {
                Some("--listen") => {
                    listen = Some(
                        value
                            .to_str()
                            .ok_or("invalid listen")?
                            .parse::<SocketAddr>()?,
                    )
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
                Some("--origin") => {
                    origin = Some(value.into_string().map_err(|_| "invalid origin")?)
                }
                _ => return Err("unknown option".into()),
            }
        }
        let listen = listen.ok_or("--listen is required")?;
        let host = host.ok_or("--host is required")?;
        if !listen.ip().is_loopback() || !host.ip().is_loopback() {
            return Err("listen and host must use loopback addresses".into());
        }
        let origin: String = origin.ok_or("--origin is required")?;
        if !origin.starts_with("http://localhost:")
            && !origin.starts_with("http://127.0.0.1:")
            && !origin.starts_with("https://localhost:")
            && !origin.starts_with("https://127.0.0.1:")
        {
            return Err("origin must be an explicit localhost origin".into());
        }
        Ok(Self {
            listen,
            host,
            server_name: server_name.ok_or("--server-name is required")?,
            certificate: certificate.ok_or("--certificate is required")?,
            origin,
        })
    }
}

#[derive(Clone)]
struct Bridge {
    endpoint: quinn::Endpoint,
    host: SocketAddr,
    server_name: String,
    origin: String,
    slots: Arc<Semaphore>,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let options = Options::parse(std::env::args_os().skip(1))?;
    let certificate = CertificateDer::from(std::fs::read(&options.certificate)?);
    let mut roots = rustls::RootCertStore::empty();
    roots.add(certificate)?;
    let mut tls = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])?
    .with_root_certificates(roots)
    .with_no_client_auth();
    tls.alpn_protocols = vec![lapis_wire::REMOTE_QUIC_PROTOCOL.to_vec()];
    tls.enable_early_data = false;
    let mut endpoint = quinn::Endpoint::client(
        if options.host.is_ipv4() {
            "127.0.0.1:0"
        } else {
            "[::1]:0"
        }
        .parse()?,
    )?;
    endpoint.set_default_client_config(quinn::ClientConfig::new(Arc::new(
        QuicClientConfig::try_from(tls)?,
    )));
    let bridge = Bridge {
        endpoint,
        host: options.host,
        server_name: options.server_name,
        origin: options.origin,
        slots: Arc::new(Semaphore::new(128)),
    };
    let listener = tokio::net::TcpListener::bind(options.listen).await?;
    eprintln!("web-bridge listening on {}", listener.local_addr()?);
    axum::serve(
        listener,
        Router::new()
            .route(lapis_wire::REMOTE_WEBSOCKET_PATH, get(upgrade))
            .with_state(bridge),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    Ok(())
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
    let socket = socket.protocols([lapis_wire::REMOTE_WEBSOCKET_PROTOCOL]);
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
    let connection = tokio::time::timeout(
        Duration::from_secs(10),
        bridge.endpoint.connect(bridge.host, &bridge.server_name)?,
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
    fn options_reject_non_loopback_endpoints() {
        let args = [
            "--listen",
            "0.0.0.0:8080",
            "--host",
            "127.0.0.1:5000",
            "--server-name",
            "localhost",
            "--certificate",
            "cert.der",
            "--origin",
            "http://localhost:5173",
        ];
        assert!(Options::parse(args.map(Into::into).into_iter()).is_err());
        let args = [
            "--listen",
            "127.0.0.1:8080",
            "--host",
            "192.0.2.1:5000",
            "--server-name",
            "localhost",
            "--certificate",
            "cert.der",
            "--origin",
            "http://localhost:5173",
        ];
        assert!(Options::parse(args.map(Into::into).into_iter()).is_err());
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
        server_tls.alpn_protocols = vec![lapis_wire::REMOTE_QUIC_PROTOCOL.to_vec()];
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
            tokio::time::sleep(Duration::from_millis(100)).await;
        });
        let mut roots = rustls::RootCertStore::empty();
        roots.add(certificate).unwrap();
        let mut tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
        tls.alpn_protocols = vec![lapis_wire::REMOTE_QUIC_PROTOCOL.to_vec()];
        let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        endpoint.set_default_client_config(quinn::ClientConfig::new(Arc::new(
            QuicClientConfig::try_from(tls).unwrap(),
        )));
        let bridge = Bridge {
            endpoint,
            host: host_address,
            server_name: "localhost".into(),
            origin: "http://localhost:5173".into(),
            slots: Arc::new(Semaphore::new(2)),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .route(lapis_wire::REMOTE_WEBSOCKET_PATH, get(upgrade))
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
            lapis_wire::REMOTE_WEBSOCKET_PROTOCOL.parse().unwrap(),
        );
        assert!(tokio_tungstenite::connect_async(bad).await.is_err());
        let mut good = url.as_str().into_client_request().unwrap();
        good.headers_mut()
            .insert("origin", "http://localhost:5173".parse().unwrap());
        good.headers_mut().insert(
            "sec-websocket-protocol",
            lapis_wire::REMOTE_WEBSOCKET_PROTOCOL.parse().unwrap(),
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
        host_task.await.unwrap();
        task.abort();
    }
}
