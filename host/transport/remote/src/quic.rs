use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use futures_util::{SinkExt, StreamExt};
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use rustls::pki_types::CertificateDer;
use tokio::{
    task::{JoinHandle, JoinSet},
    time::timeout,
};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

use crate::{
    ClientMessage, RemoteLimits, RemoteRequestHandler, ServerMessage, SharedRemoteAuth,
    Tls13ServerConfig,
    rate_limit::AuthenticationAttemptLimiter,
    server::{EndpointState, handle_socket},
    socket::RemoteSocket,
    webtransport::{self, SessionError},
};

pub const QUIC_PROTOCOL: &[u8] = wire::REMOTE_QUIC_PROTOCOL;
type Error = Box<dyn std::error::Error + Send + Sync>;

/// 呼出元のTokio runtime上で動作する。接続終了はHostの終了を意味しない。
pub struct QuicServer {
    endpoint: quinn::Endpoint,
    task: Option<JoinHandle<()>>,
}

impl QuicServer {
    pub async fn bind(
        address: SocketAddr,
        tls: Tls13ServerConfig,
        auth: SharedRemoteAuth,
        handler: Arc<dyn RemoteRequestHandler>,
        limits: RemoteLimits,
    ) -> Result<Self, Error> {
        if !auth
            .lock()
            .map_err(|_| "authentication state unavailable")?
            .is_enabled()
        {
            return Err("remote access is disabled".into());
        }
        let mut tls = (*tls.into_inner()).clone();
        tls.alpn_protocols = vec![QUIC_PROTOCOL.to_vec()];
        tls.max_early_data_size = 0;
        let mut config =
            quinn::ServerConfig::with_crypto(Arc::new(QuicServerConfig::try_from(tls)?));
        let mut transport = quinn::TransportConfig::default();
        // Desktop は 1 本の bidi stream を使い、ブラウザーの WebTransport は
        // 制御・QPACK・CONNECT の uni/bidi stream を開くため、両方を許可する。
        transport.max_concurrent_bidi_streams(8u32.into());
        transport.max_concurrent_uni_streams(8u32.into());
        transport.max_idle_timeout(Some(limits.idle_timeout().duration().try_into()?));
        config.transport_config(Arc::new(transport));
        let endpoint = quinn::Endpoint::server(config, address)?;
        let listener = endpoint.clone();
        let state = EndpointState {
            auth,
            handler,
            limits,
            authentication_attempts: AuthenticationAttemptLimiter::new(
                limits.authentication_rate_limit(),
            ),
        };
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    incoming = listener.accept() => {
                        let Some(incoming) = incoming else { break };
                        let peer = incoming.remote_address();
                        if connections.len() >= 128 || !state.authentication_attempts.allows(peer.ip(), Instant::now()) {
                            incoming.refuse();
                            continue;
                        }
                        let state = state.clone();
                        connections.spawn(accept_connection(incoming, state, peer));
                    }
                    _ = connections.join_next(), if !connections.is_empty() => {}
                }
            }
            connections.abort_all();
            while connections.join_next().await.is_some() {}
        });
        Ok(Self {
            endpoint,
            task: Some(task),
        })
    }

    pub fn listening_address(&self) -> Result<SocketAddr, std::io::Error> {
        self.endpoint.local_addr()
    }

    pub async fn shutdown(mut self) {
        self.endpoint.close(0u32.into(), b"host shutting down");
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
        self.endpoint.wait_idle().await;
    }
}

impl Drop for QuicServer {
    fn drop(&mut self) {
        self.endpoint.close(0u32.into(), b"host shutting down");
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

fn codec(limits: RemoteLimits) -> tokio_util::codec::length_delimited::Builder {
    let mut builder = LengthDelimitedCodec::builder();
    builder.max_frame_length(limits.max_message_size().bytes());
    builder
}

/// 1 接続の最初の stream で経路を決める。
///
/// Desktop は接続直後に bidi stream を 1 本開く。ブラウザーの WebTransport は
/// 接続直後に制御用の uni stream を開くため、どちらが先に来たかで経路を選ぶ。
/// 経路を選んだら、その接続ではもう一方の形式を扱わない。
async fn accept_connection(incoming: quinn::Incoming, state: EndpointState, peer: SocketAddr) {
    let setup = webtransport::SETUP_TIMEOUT;
    let connection = match tokio::time::timeout(setup, incoming).await {
        Ok(Ok(connection)) => connection,
        _ => {
            state
                .authentication_attempts
                .record_failure(peer.ip(), Instant::now());
            return;
        }
    };
    let first = tokio::select! {
        biased;
        stream = connection.accept_bi() => First::Quic(stream),
        stream = connection.accept_uni() => First::WebTransport(stream),
    };
    match first {
        First::Quic(Ok((send, recv))) => {
            let reader = codec(state.limits).new_read(recv);
            let writer = codec(state.limits).new_write(send);
            handle_socket(
                RemoteSocket::Quic {
                    connection,
                    reader,
                    writer,
                },
                state,
                peer,
            )
            .await;
        }
        First::Quic(Err(_)) => {}
        First::WebTransport(Ok(control)) => {
            if serve_webtransport(connection, control, &state, peer)
                .await
                .is_err()
            {
                state
                    .authentication_attempts
                    .record_failure(peer.ip(), Instant::now());
            }
        }
        First::WebTransport(Err(_)) => {}
    }
}

enum First {
    Quic(Result<(quinn::SendStream, quinn::RecvStream), quinn::ConnectionError>),
    WebTransport(Result<quinn::RecvStream, quinn::ConnectionError>),
}

/// ブラウザーの WebTransport 接続を、1 セッション = 1 Session として処理する。
///
/// 同一 QUIC 接続で複数セッションを張れるが、Host では 1 接続 1 セッションに
/// 限定する。再接続は新しい接続で行い、Session と attachment の寿命は
/// Desktop と同じ規則で管理する。
async fn serve_webtransport(
    connection: quinn::Connection,
    control: quinn::RecvStream,
    state: &EndpointState,
    peer: SocketAddr,
) -> Result<(), SessionError> {
    let setup = webtransport::SETUP_TIMEOUT;
    let (mut send, recv, request) = timeout(setup, async {
        // 制御ストリームの SETTINGS と CONNECT ストリームの request を並行に待つ。
        // Host 側も自分の SETTINGS を開いて広告する。
        let settings = async {
            let (read, write) = futures_util::join!(
                webtransport::read_settings(control),
                webtransport::write_settings(&connection)
            );
            read?;
            write
        };
        let request = async {
            let (send, mut recv) = connection
                .accept_bi()
                .await
                .map_err(|_| SessionError::Transport)?;
            let request = webtransport::read_connect(&mut recv).await?;
            Ok::<_, SessionError>((send, recv, request))
        };
        let (settings, request) = futures_util::join!(settings, request);
        settings?;
        request
    })
    .await
    .map_err(|_| SessionError::Transport)??;
    // ホスト名や path は認証に使わない。認可は既存の credential と capability が決まる。
    let _ = &request;
    timeout(setup, webtransport::write_connect_ok(&mut send))
        .await
        .map_err(|_| SessionError::Transport)??;
    let (reader, writer) = webtransport::ConnectStream { send, recv }.split();
    let reader = codec(state.limits).new_read(reader);
    let writer = codec(state.limits).new_write(writer);
    handle_socket(
        RemoteSocket::WebTransport { reader, writer },
        state.clone(),
        peer,
    )
    .await;
    Ok(())
}

/// 証明書検証を省略しないQUIC接続。LocalもRemoteも同じ接続経路を使う。
pub struct QuicClient {
    endpoint: quinn::Endpoint,
    connection: quinn::Connection,
    reader: FramedRead<quinn::RecvStream, LengthDelimitedCodec>,
    writer: FramedWrite<quinn::SendStream, LengthDelimitedCodec>,
    request_timeout: Duration,
}

impl QuicClient {
    pub async fn connect(
        address: SocketAddr,
        server_name: &str,
        certificate: CertificateDer<'static>,
        limits: RemoteLimits,
    ) -> Result<Self, Error> {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(certificate)?;
        let mut tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_root_certificates(roots)
        .with_no_client_auth();
        tls.alpn_protocols = vec![QUIC_PROTOCOL.to_vec()];
        tls.enable_early_data = false;
        let config = quinn::ClientConfig::new(Arc::new(QuicClientConfig::try_from(tls)?));
        let bind = if address.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        };
        let mut endpoint = quinn::Endpoint::client(bind.parse()?)?;
        endpoint.set_default_client_config(config);
        let connection = timeout(
            limits.authentication_timeout().duration(),
            endpoint.connect(address, server_name)?,
        )
        .await??;
        let (send, recv) = connection.open_bi().await?;
        Ok(Self {
            endpoint,
            connection,
            reader: codec(limits).new_read(recv),
            writer: codec(limits).new_write(send),
            request_timeout: limits.request_timeout().duration(),
        })
    }

    pub async fn send(&mut self, message: &ClientMessage) -> Result<(), Error> {
        let bytes = serde_json::to_vec(message)?;
        // 書込途中のtimeout後は同じstreamを再利用しない。
        match timeout(self.request_timeout, self.writer.send(bytes.into())).await {
            Ok(Ok(())) => Ok(()),
            result => {
                self.connection.close(1u32.into(), b"send failed");
                match result {
                    Ok(Err(error)) => Err(error.into()),
                    _ => Err("send timeout".into()),
                }
            }
        }
    }

    /// イベントも応答も順序どおり返す。取消しても部分フレームは保持する。
    pub async fn recv(&mut self) -> Result<ServerMessage, Error> {
        let bytes = self.reader.next().await.ok_or("connection closed")??;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub async fn shutdown(self) {
        self.connection.close(0u32.into(), b"client disconnected");
        self.endpoint.close(0u32.into(), b"client disconnected");
        // 独立runtimeを破棄する前にclose packetを送る時間を確保する。
        let _ = timeout(Duration::from_secs(1), self.endpoint.wait_idle()).await;
    }
}

impl Drop for QuicClient {
    fn drop(&mut self) {
        self.connection.close(0u32.into(), b"client disconnected");
        self.endpoint.close(0u32.into(), b"client disconnected");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AuthConfig, AuthPolicy, CredentialLifetime, PairingLifetime, RemoteAuth,
        RemoteResponseFuture, SessionGrant,
    };

    struct NoRequests;
    impl RemoteRequestHandler for NoRequests {
        fn handle(
            &self,
            _: SessionGrant,
            _: client_api::RequestEnvelope,
        ) -> RemoteResponseFuture<'_> {
            Box::pin(async { panic!("invalid frames must not reach the runtime") })
        }
    }

    #[test]
    fn disabled_quic_cannot_listen_and_invalid_frames_never_dispatch() {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let auth = Arc::new(std::sync::Mutex::new(RemoteAuth::system(
                    AuthConfig::new(
                        PairingLifetime::new(10).unwrap(),
                        CredentialLifetime::new(20).unwrap(),
                    ),
                    AuthPolicy::new(Default::default()),
                )));
                let certified =
                    rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
                let cert = certified.cert.der().clone();
                let tls = crate::tls13_server_config(
                    vec![cert.clone()],
                    rustls::pki_types::PrivateKeyDer::Pkcs8(
                        certified.signing_key.serialize_der().into(),
                    ),
                )
                .unwrap();
                assert!(
                    QuicServer::bind(
                        "127.0.0.1:0".parse().unwrap(),
                        tls.clone(),
                        auth.clone(),
                        Arc::new(NoRequests),
                        RemoteLimits::default()
                    )
                    .await
                    .is_err()
                );
                auth.lock().unwrap().enable();
                let server = QuicServer::bind(
                    "127.0.0.1:0".parse().unwrap(),
                    tls,
                    auth,
                    Arc::new(NoRequests),
                    RemoteLimits::default(),
                )
                .await
                .unwrap();
                let address = server.listening_address().unwrap();
                let mut invalid_utf8 = QuicClient::connect(
                    address,
                    "localhost",
                    cert.clone(),
                    RemoteLimits::default(),
                )
                .await
                .unwrap();
                invalid_utf8.writer.send(vec![0xff].into()).await.unwrap();
                assert!(
                    timeout(Duration::from_secs(2), invalid_utf8.recv())
                        .await
                        .unwrap()
                        .is_err()
                );
                let mut oversized =
                    QuicClient::connect(address, "localhost", cert, RemoteLimits::default())
                        .await
                        .unwrap();
                // 宣言長だけで拒否し、payloadの受信や大容量確保を待たない。
                let length =
                    (RemoteLimits::default().max_message_size().bytes() as u32 + 1).to_be_bytes();
                oversized.writer.get_mut().write_all(&length).await.unwrap();
                assert!(
                    timeout(Duration::from_secs(2), oversized.recv())
                        .await
                        .unwrap()
                        .is_err()
                );
                server.shutdown().await;
            });
    }

    /// WebTransport のセッション確立を Host と Client の両側で検証する。
    /// ブラウザーを起動せずに、CONNECT 応答とフレームの往復まで確認する。
    #[test]
    fn webtransport_session_accepts_frames_on_the_connect_stream() {
        use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let auth = Arc::new(std::sync::Mutex::new(RemoteAuth::system(
                    AuthConfig::new(
                        PairingLifetime::new(10).unwrap(),
                        CredentialLifetime::new(20).unwrap(),
                    ),
                    AuthPolicy::new(Default::default()),
                )));
                auth.lock().unwrap().enable();
                let certified =
                    rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
                let cert = certified.cert.der().clone();
                let tls = crate::tls13_server_config(
                    vec![cert.clone()],
                    rustls::pki_types::PrivateKeyDer::Pkcs8(
                        certified.signing_key.serialize_der().into(),
                    ),
                )
                .unwrap();
                let server = QuicServer::bind(
                    "127.0.0.1:0".parse().unwrap(),
                    tls,
                    auth,
                    Arc::new(NoRequests),
                    RemoteLimits::default(),
                )
                .await
                .unwrap();
                let address = server.listening_address().unwrap();

                let mut roots = rustls::RootCertStore::empty();
                roots.add(cert).unwrap();
                let mut tls = rustls::ClientConfig::builder_with_provider(Arc::new(
                    rustls::crypto::aws_lc_rs::default_provider(),
                ))
                .with_protocol_versions(&[&rustls::version::TLS13])
                .unwrap()
                .with_root_certificates(roots)
                .with_no_client_auth();
                tls.alpn_protocols = vec![QUIC_PROTOCOL.to_vec()];
                let config =
                    quinn::ClientConfig::new(Arc::new(QuicClientConfig::try_from(tls).unwrap()));
                let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
                endpoint.set_default_client_config(config);
                let connection = endpoint
                    .connect(address, "localhost")
                    .unwrap()
                    .await
                    .unwrap();

                let session = timeout(
                    Duration::from_secs(5),
                    crate::connect_session(&connection, "localhost"),
                )
                .await
                .unwrap()
                .expect("server accepts the WebTransport session");
                let (reader, writer) = session.split();
                let mut reader = FramedRead::new(reader, LengthDelimitedCodec::new());
                let mut writer = FramedWrite::new(writer, LengthDelimitedCodec::new());

                // 未認証の要求はセッションが拒否し、応答を返して接続を閉じる。
                let message = ClientMessage::Request(client_api::RequestEnvelope {
                    request_id: client_api::RequestId::try_new("unauthenticated").unwrap(),
                    body: client_api::RequestBody::WorkspaceList(Default::default()),
                });
                writer
                    .send(serde_json::to_vec(&message).unwrap().into())
                    .await
                    .unwrap();
                let reply = timeout(Duration::from_secs(5), reader.next())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                let reply: ServerMessage = serde_json::from_slice(&reply).unwrap();
                assert!(matches!(
                    reply,
                    ServerMessage::Error(error) if error.code.as_str() == client_api::UNAUTHORIZED
                ));
                endpoint.close(0u32.into(), b"test done");
                server.shutdown().await;
            });
    }
}
