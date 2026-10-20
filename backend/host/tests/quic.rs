use lapis_client_api::*;
use lapis_host::BackendHost;
use lapis_remote::*;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

struct Fixture {
    _root: tempfile::TempDir,
    host: BackendHost,
    workspace: WorkspaceId,
    auth: SharedRemoteAuth,
    cert: CertificateDer<'static>,
    server: QuicServer,
}

impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let host = BackendHost::start().unwrap();
        let workspace = host.register_workspace(root.path()).unwrap();
        let auth = Arc::new(Mutex::new(RemoteAuth::system(
            AuthConfig::new(
                PairingLifetime::new(300).unwrap(),
                CredentialLifetime::new(3600).unwrap(),
            ),
            AuthPolicy::new(lapis_host::supported_capabilities()),
        )));
        auth.lock().unwrap().enable();
        let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        let cert = certified.cert.der().clone();
        let tls = tls13_server_config(
            vec![cert.clone()],
            PrivateKeyDer::Pkcs8(certified.signing_key.serialize_der().into()),
        )
        .unwrap();
        let server = QuicServer::bind(
            "127.0.0.1:0".parse().unwrap(),
            tls,
            auth.clone(),
            Arc::new(BackendRemoteHandler::new(host.service())),
            RemoteLimits::default(),
        )
        .await
        .unwrap();
        Self {
            _root: root,
            host,
            workspace,
            auth,
            cert,
            server,
        }
    }
    fn address(&self) -> SocketAddr {
        self.server.listening_address().unwrap()
    }
    async fn connect(&self) -> QuicClient {
        QuicClient::connect(
            self.address(),
            "localhost",
            self.cert.clone(),
            RemoteLimits::default(),
        )
        .await
        .unwrap()
    }
    async fn pair(&self, id: &str) -> (QuicClient, PairedResponse) {
        let mut client = self.connect().await;
        let token = self
            .auth
            .lock()
            .unwrap()
            .begin_pairing(self.workspace.clone())
            .unwrap();
        client
            .send(&ClientMessage::Pair(PairRequest::new(
                hello(id),
                token.secret_bytes(),
            )))
            .await
            .unwrap();
        let ServerMessage::Paired(paired) = receive(&mut client).await else {
            panic!("pairing failed")
        };
        (client, paired)
    }
}

fn hello(id: &str) -> ClientHello {
    ClientHello {
        protocol: ProtocolRange::exact(CURRENT_PROTOCOL),
        client_id: ClientId::try_new(id).unwrap(),
        client_name: id.to_owned(),
        client_kind: ClientKind::Desktop,
        requested_capabilities: lapis_host::supported_capabilities(),
    }
}

async fn receive(client: &mut QuicClient) -> ServerMessage {
    tokio::time::timeout(Duration::from_secs(10), client.recv())
        .await
        .unwrap()
        .unwrap()
}

async fn request(client: &mut QuicClient, number: u64, body: RequestBody) -> ResponseBody {
    let id = RequestId::try_new(format!("request-{number}")).unwrap();
    client
        .send(&ClientMessage::Request(RequestEnvelope {
            request_id: id.clone(),
            body,
        }))
        .await
        .unwrap();
    loop {
        match receive(client).await {
            ServerMessage::Response(response) => {
                assert_eq!(response.request_id, id);
                return response.body;
            }
            ServerMessage::Event(_) => {}
            _ => panic!("unexpected message"),
        }
    }
}

async fn connect_workspace(client: &mut QuicClient, workspace: &WorkspaceId) -> WorkspaceSnapshot {
    let ResponseBody::WorkspaceConnect(_) = request(
        client,
        1,
        RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
            workspace_id: workspace.clone(),
        }),
    )
    .await
    else {
        panic!("workspace connect failed")
    };
    let ResponseBody::SnapshotResync(response) = request(
        client,
        0,
        RequestBody::SnapshotResync(SnapshotRequest {
            workspace_id: workspace.clone(),
            reason: SnapshotReason::Initial,
        }),
    )
    .await
    else {
        panic!("snapshot failed")
    };
    response.snapshot
}

fn run(test: impl std::future::Future<Output = ()>) {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
        .block_on(test);
}

#[test]
fn quic_requires_trusted_certificate_and_matching_server_name() {
    run(async {
        let fixture = Fixture::new().await;
        let other = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        assert!(
            QuicClient::connect(
                fixture.address(),
                "localhost",
                other.cert.der().clone(),
                RemoteLimits::default()
            )
            .await
            .is_err()
        );
        assert!(
            QuicClient::connect(
                fixture.address(),
                "wrong-host",
                fixture.cert.clone(),
                RemoteLimits::default()
            )
            .await
            .is_err()
        );
        let _client = fixture.connect().await;
    });
}

#[test]
fn quic_rejects_unauthenticated_requests_and_reused_pairing_tokens() {
    run(async {
        let fixture = Fixture::new().await;
        let mut unauthenticated = fixture.connect().await;
        unauthenticated
            .send(&ClientMessage::Request(RequestEnvelope {
                request_id: RequestId::try_new("request").unwrap(),
                body: RequestBody::WorkspaceList(WorkspaceListRequest {}),
            }))
            .await
            .unwrap();
        assert!(
            matches!(receive(&mut unauthenticated).await, ServerMessage::Error(error) if error.code.as_str() == UNAUTHORIZED)
        );
        let token = fixture
            .auth
            .lock()
            .unwrap()
            .begin_pairing(fixture.workspace.clone())
            .unwrap();
        let mut first = fixture.connect().await;
        first
            .send(&ClientMessage::Pair(PairRequest::new(
                hello("first"),
                token.secret_bytes(),
            )))
            .await
            .unwrap();
        assert!(matches!(
            receive(&mut first).await,
            ServerMessage::Paired(_)
        ));
        let mut second = fixture.connect().await;
        second
            .send(&ClientMessage::Pair(PairRequest::new(
                hello("second"),
                token.secret_bytes(),
            )))
            .await
            .unwrap();
        assert!(
            matches!(receive(&mut second).await, ServerMessage::Error(error) if error.code.as_str() == UNAUTHORIZED)
        );
    });
}

#[test]
fn quic_reconnect_restores_dirty_documents_and_rejects_revoked_credentials() {
    run(async {
        let fixture = Fixture::new().await;
        let (mut first, mut paired) = fixture.pair("first").await;
        connect_workspace(&mut first, &fixture.workspace).await;
        let ResponseBody::DocumentCreate(created) = request(
            &mut first,
            2,
            RequestBody::DocumentCreate(DocumentCreateRequest {
                workspace_id: fixture.workspace.clone(),
                path: WorkspaceRelativePath::parse("draft.ts").unwrap(),
                encoding: DocumentEncoding::Utf8,
                content: "const dirty = true;".to_owned(),
            }),
        )
        .await
        else {
            panic!("create failed")
        };
        assert!(created.document.dirty);
        let credential = paired.credential_id().unwrap();
        let secret = zeroize::Zeroizing::new(paired.take_secret().unwrap());
        drop(first);
        let mut reconnected = fixture.connect().await;
        reconnected
            .send(&ClientMessage::Authenticate(AuthenticateRequest::new(
                hello("first"),
                fixture.workspace.clone(),
                &credential,
                &secret,
            )))
            .await
            .unwrap();
        assert!(matches!(
            receive(&mut reconnected).await,
            ServerMessage::Authenticated(_)
        ));
        let snapshot = connect_workspace(&mut reconnected, &fixture.workspace).await;
        assert!(
            snapshot
                .documents
                .iter()
                .any(
                    |document| document.document_id == created.document.document_id
                        && document.dirty
                        && document.content == "const dirty = true;"
                )
        );
        fixture.auth.lock().unwrap().revoke(&credential);
        let ResponseBody::Error(error) = request(
            &mut reconnected,
            3,
            RequestBody::SnapshotResync(SnapshotRequest {
                workspace_id: fixture.workspace.clone(),
                reason: SnapshotReason::Reconnect,
            }),
        )
        .await
        else {
            panic!("revoked session must fail")
        };
        assert_eq!(error.code.as_str(), UNAUTHORIZED);
        let mut rejected = fixture.connect().await;
        rejected
            .send(&ClientMessage::Authenticate(AuthenticateRequest::new(
                hello("first"),
                fixture.workspace.clone(),
                &credential,
                &secret,
            )))
            .await
            .unwrap();
        assert!(
            matches!(receive(&mut rejected).await, ServerMessage::Error(error) if error.code.as_str() == UNAUTHORIZED)
        );
    });
}

#[test]
fn revoked_client_cannot_receive_further_document_events() {
    run(async {
        let fixture = Fixture::new().await;
        let (mut revoked, paired) = fixture.pair("revoked").await;
        connect_workspace(&mut revoked, &fixture.workspace).await;
        let (mut writer, _) = fixture.pair("writer").await;
        connect_workspace(&mut writer, &fixture.workspace).await;
        fixture
            .auth
            .lock()
            .unwrap()
            .revoke(&paired.credential_id().unwrap());
        assert!(matches!(
            request(
                &mut writer,
                2,
                RequestBody::DocumentCreate(DocumentCreateRequest {
                    workspace_id: fixture.workspace.clone(),
                    path: WorkspaceRelativePath::parse("private.ts").unwrap(),
                    encoding: DocumentEncoding::Utf8,
                    content: "private content".to_owned(),
                })
            )
            .await,
            ResponseBody::DocumentCreate(_)
        ));
        assert!(
            matches!(receive(&mut revoked).await, ServerMessage::Error(error) if error.code.as_str() == UNAUTHORIZED)
        );
    });
}

#[test]
fn shutting_down_quic_cleans_session_attachments_before_returning() {
    run(async {
        let fixture = Fixture::new().await;
        let (mut client, paired) = fixture.pair("shutdown").await;
        connect_workspace(&mut client, &fixture.workspace).await;
        let session = paired.hello.session_id.clone();
        fixture.server.shutdown().await;
        assert!(!fixture.auth.lock().unwrap().disconnect(&session));
    });
}

#[test]
fn websocket_and_quic_keep_the_same_authenticated_runtime() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::{
        Connector,
        tungstenite::{Message, client::IntoClientRequest},
    };
    run(async {
        let fixture = Fixture::new().await;
        let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        let tls = tls13_server_config(
            vec![certified.cert.der().clone()],
            PrivateKeyDer::Pkcs8(certified.signing_key.serialize_der().into()),
        )
        .unwrap();
        let server = RemoteServer::start(
            RemoteServerConfig::new("127.0.0.1:0".parse().unwrap(), tls, RemoteLimits::default()),
            fixture.auth.clone(),
            Arc::new(BackendRemoteHandler::new(fixture.host.service())),
        )
        .unwrap();
        let mut roots = rustls::RootCertStore::empty();
        roots.add(certified.cert.der().clone()).unwrap();
        let client_tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
        let mut request = format!(
            "wss://localhost:{}/remote",
            server.listening_address().port()
        )
        .into_client_request()
        .unwrap();
        request.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            REMOTE_WEBSOCKET_PROTOCOL.parse().unwrap(),
        );
        let (mut socket, _) = tokio_tungstenite::connect_async_tls_with_config(
            request,
            None,
            false,
            Some(Connector::Rustls(Arc::new(client_tls))),
        )
        .await
        .unwrap();
        let token = fixture
            .auth
            .lock()
            .unwrap()
            .begin_pairing(fixture.workspace.clone())
            .unwrap();
        socket
            .send(Message::text(
                serde_json::to_string(&ClientMessage::Pair(PairRequest::new(
                    hello("websocket"),
                    token.secret_bytes(),
                )))
                .unwrap(),
            ))
            .await
            .unwrap();
        let paired = socket.next().await.unwrap().unwrap().into_text().unwrap();
        assert!(matches!(
            serde_json::from_str::<ServerMessage>(&paired).unwrap(),
            ServerMessage::Paired(_)
        ));
        socket
            .send(Message::text(
                serde_json::to_string(&ClientMessage::Request(RequestEnvelope {
                    request_id: RequestId::try_new("connect").unwrap(),
                    body: RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
                        workspace_id: fixture.workspace.clone(),
                    }),
                }))
                .unwrap(),
            ))
            .await
            .unwrap();
        assert!(matches!(
            serde_json::from_str::<ServerMessage>(
                &socket.next().await.unwrap().unwrap().into_text().unwrap()
            )
            .unwrap(),
            ServerMessage::Response(ResponseEnvelope {
                body: ResponseBody::WorkspaceConnect(_),
                ..
            })
        ));
        let (mut quic, _) = fixture.pair("quic").await;
        connect_workspace(&mut quic, &fixture.workspace).await;
        assert!(matches!(
            request_document(&mut quic, &fixture.workspace).await,
            ResponseBody::DocumentCreate(_)
        ));
        let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .into_text()
            .unwrap();
        assert!(matches!(
            serde_json::from_str::<ServerMessage>(&message).unwrap(),
            ServerMessage::Event(EventEnvelope {
                body: EventBody::DocumentReplaced { .. },
                ..
            })
        ));
        drop(socket);
        server.shutdown().unwrap();
    });
}

async fn request_document(client: &mut QuicClient, workspace: &WorkspaceId) -> ResponseBody {
    request(
        client,
        2,
        RequestBody::DocumentCreate(DocumentCreateRequest {
            workspace_id: workspace.clone(),
            path: WorkspaceRelativePath::parse("shared.ts").unwrap(),
            encoding: DocumentEncoding::Utf8,
            content: "shared".to_owned(),
        }),
    )
    .await
}

#[test]
fn quic_and_embedded_clients_share_live_pty_across_disconnect() {
    run(async {
        let fixture = Fixture::new().await;
        let (mut first, _) = fixture.pair("first").await;
        connect_workspace(&mut first, &fixture.workspace).await;
        let ResponseBody::TerminalStart(started) = request(
            &mut first,
            2,
            RequestBody::TerminalStart(TerminalStartRequest {
                workspace_id: fixture.workspace.clone(),
                cwd: None,
                command: None,
                size: TerminalSize {
                    columns: 80,
                    rows: 24,
                },
            }),
        )
        .await
        else {
            panic!("start failed")
        };
        let terminal = started.terminal.terminal_id;
        let (mut second, _) = fixture.pair("second").await;
        let snapshot = connect_workspace(&mut second, &fixture.workspace).await;
        assert!(
            snapshot
                .terminals
                .iter()
                .any(|value| value.terminal_id == terminal
                    && value.status == TerminalStatus::Running)
        );
        drop(first);
        assert!(matches!(
            request(
                &mut second,
                2,
                RequestBody::TerminalInput(TerminalInputRequest {
                    terminal_id: terminal.clone(),
                    data: if cfg!(windows) {
                        "Write-Output ('LAPIS_QUIC_' + 'SHARED_PTY')\r\n"
                    } else {
                        "printf 'LAPIS_QUIC_%s\\n' SHARED_PTY\n"
                    }
                    .to_owned(),
                })
            )
            .await,
            ResponseBody::TerminalInput(_)
        ));
        tokio::time::timeout(Duration::from_secs(15), async {
            let mut number = 3;
            loop {
                let ResponseBody::SnapshotResync(response) = request(
                    &mut second,
                    number,
                    RequestBody::SnapshotResync(SnapshotRequest {
                        workspace_id: fixture.workspace.clone(),
                        reason: SnapshotReason::Reconnect,
                    }),
                )
                .await
                else {
                    panic!("snapshot failed")
                };
                let value = response
                    .snapshot
                    .terminals
                    .iter()
                    .find(|value| value.terminal_id == terminal)
                    .unwrap();
                assert_eq!(value.status, TerminalStatus::Running);
                if String::from_utf8_lossy(&value.buffered_output).contains("LAPIS_QUIC_SHARED_PTY")
                {
                    break;
                }
                number += 1;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        // Local埋め込み接続も同じHostのPTYを再起動せず発見する。
        let embedded = fixture.host.connect(fixture._root.path()).unwrap();
        let ResponseBody::SnapshotResync(response) = embedded
            .request(RequestBody::SnapshotResync(SnapshotRequest {
                workspace_id: fixture.workspace.clone(),
                reason: SnapshotReason::Reconnect,
            }))
            .unwrap()
        else {
            panic!("embedded snapshot failed")
        };
        assert!(
            response
                .snapshot
                .terminals
                .iter()
                .any(|value| value.terminal_id == terminal)
        );
        assert!(matches!(
            request(
                &mut second,
                9999,
                RequestBody::TerminalTerminate(TerminalTerminateRequest {
                    terminal_id: terminal
                })
            )
            .await,
            ResponseBody::TerminalTerminate(_)
        ));
    });
}
