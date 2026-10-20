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
};

pub const QUIC_PROTOCOL: &[u8] = b"lapis.v0";
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
        transport.max_concurrent_bidi_streams(1u32.into());
        transport.max_concurrent_uni_streams(0u32.into());
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
                        connections.spawn(async move {
                            let handshake = async {
                                let connection = incoming.await?;
                                let (send, recv) = connection.accept_bi().await?;
                                Ok::<_, Error>((connection, send, recv))
                            };
                            let Ok(Ok((connection, send, recv))) = timeout(state.limits.authentication_timeout().duration(), handshake).await else {
                                state.authentication_attempts.record_failure(peer.ip(), Instant::now());
                                return;
                            };
                            let reader = codec(state.limits).new_read(recv);
                            let writer = codec(state.limits).new_write(send);
                            handle_socket(RemoteSocket::Quic { connection, reader, writer }, state, peer).await;
                        });
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
            _: lapis_client_api::RequestEnvelope,
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
}
