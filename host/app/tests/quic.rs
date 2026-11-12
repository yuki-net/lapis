use client_api::*;
use host::BackendHost;
use remote::*;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

struct Fixture {
    _root: tempfile::TempDir,
    _state: tempfile::TempDir,
    host: BackendHost,
    workspace: WorkspaceId,
    auth: SharedRemoteAuth,
    cert: CertificateDer<'static>,
    server: QuicServer,
}

impl Fixture {
    async fn config(&self, id: &str) -> QuicConnectionConfig {
        let (client, mut paired) = self.pair(id).await;
        drop(client);
        QuicConnectionConfig {
            address: self.address(),
            server_name: "localhost".to_owned(),
            certificate: self.cert.clone(),
            hello: hello(id),
            workspace_id: Some(self.workspace.clone()),
            host_id: paired.hello.host_id.clone(),
            auth: QuicAuth::Credential(Arc::new(CredentialHandle::from_parts(
                paired.credential_id().unwrap(),
                &mut paired.take_secret().unwrap(),
            ))),
        }
    }
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let host = BackendHost::start_with_state_directory(state.path().to_path_buf()).unwrap();
        let workspace = host.register_workspace(root.path()).unwrap();
        let mut auth = RemoteAuth::system(
            AuthConfig::new(
                PairingLifetime::new(300).unwrap(),
                CredentialLifetime::new(3600).unwrap(),
            ),
            AuthPolicy::new(
                CapabilitySet::try_new(
                    host::supported_capabilities()
                        .iter()
                        .cloned()
                        .chain([CapabilityId::try_new(capability::HOST_MANAGE).unwrap()]),
                )
                .unwrap(),
            ),
        );
        auth.set_workspace_resolver(Arc::new(host.clone()));
        auth.enable();
        let auth = Arc::new(Mutex::new(auth));
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
            Arc::new(host::HostRemoteHandler::new(host.clone()).with_auth(auth.clone())),
            RemoteLimits::default(),
        )
        .await
        .unwrap();
        Self {
            _root: root,
            _state: state,
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_can_invite_a_web_client_but_a_workspace_client_cannot() {
    let fixture = Fixture::new().await;
    let (mut ordinary, _) = fixture.pair("ordinary").await;
    assert!(matches!(
        request(&mut ordinary, 1, RequestBody::HostInvite(HostInviteRequest {})).await,
        ResponseBody::Error(ref error) if error.code.as_str() == FORBIDDEN
    ));

    let token = fixture
        .auth
        .lock()
        .unwrap()
        .begin_owner_pairing(fixture.workspace.clone())
        .unwrap();
    let mut owner_hello = hello("owner");
    owner_hello.requested_capabilities = CapabilitySet::try_new(
        host::supported_capabilities()
            .iter()
            .cloned()
            .chain([CapabilityId::try_new(capability::HOST_MANAGE).unwrap()]),
    )
    .unwrap();
    let mut owner = fixture.connect().await;
    owner
        .send(&ClientMessage::Pair(PairRequest::new(
            owner_hello,
            token.secret_bytes(),
        )))
        .await
        .unwrap();
    assert!(matches!(
        receive(&mut owner).await,
        ServerMessage::Paired(_)
    ));
    let ResponseBody::HostInvite(invite) =
        request(&mut owner, 2, RequestBody::HostInvite(HostInviteRequest {})).await
    else {
        panic!("owner invitation failed");
    };
    assert_eq!(invite.token.len(), 64);
    let mut secret = zeroize::Zeroizing::new([0u8; 32]);
    for (index, byte) in secret.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&invite.token[index * 2..index * 2 + 2], 16).unwrap();
    }
    let mut web = fixture.connect().await;
    let mut web_hello = hello("web-client");
    web_hello.client_kind = ClientKind::Web;
    web.send(&ClientMessage::Pair(PairRequest::new(web_hello, &secret)))
        .await
        .unwrap();
    let ServerMessage::Paired(mut paired) = receive(&mut web).await else {
        panic!("web pairing failed")
    };
    let web_id = paired.credential_id().unwrap();
    let web_secret = zeroize::Zeroizing::new(paired.take_secret().unwrap());
    assert!(matches!(
        request(
            &mut web,
            3,
            RequestBody::WorkspaceList(WorkspaceListRequest {})
        )
        .await,
        ResponseBody::WorkspaceList(_)
    ));
    assert!(
        matches!(request(&mut web, 4, RequestBody::HostInvite(HostInviteRequest {})).await,
        ResponseBody::Error(ref error) if error.code.as_str() == FORBIDDEN)
    );
    let mut reused = fixture.connect().await;
    reused
        .send(&ClientMessage::Pair(PairRequest::new(
            hello("another-web"),
            &secret,
        )))
        .await
        .unwrap();
    assert!(matches!(
        receive(&mut reused).await,
        ServerMessage::Error(_)
    ));
    let ResponseBody::HostDeviceList(list) = request(
        &mut owner,
        5,
        RequestBody::HostDeviceList(HostDeviceListRequest {}),
    )
    .await
    else {
        panic!("device list failed")
    };
    assert!(
        list.devices
            .iter()
            .any(|device| device.client_id == "owner" && device.is_current)
    );
    let device = list
        .devices
        .iter()
        .find(|device| device.client_id == "web-client")
        .unwrap();
    assert!(!device.is_current);
    assert_eq!(device.kind, "web");
    assert!(matches!(
        request(
            &mut owner,
            6,
            RequestBody::HostDeviceRevoke(HostDeviceRevokeRequest {
                device_id: device.id.clone()
            })
        )
        .await,
        ResponseBody::HostDeviceRevoke(HostDeviceRevokeResponse { revoked: true })
    ));
    assert!(
        matches!(request(&mut web, 7, RequestBody::WorkspaceList(WorkspaceListRequest {})).await,
        ResponseBody::Error(ref error) if error.code.as_str() == UNAUTHORIZED)
    );
    let mut reconnect = fixture.connect().await;
    reconnect
        .send(&ClientMessage::Authenticate(AuthenticateRequest::new_auto(
            hello("web-client"),
            &web_id,
            &web_secret,
        )))
        .await
        .unwrap();
    assert!(matches!(
        receive(&mut reconnect).await,
        ServerMessage::Error(_)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn desktop_ports_discover_and_reattach_the_same_live_terminal_with_its_cwd() {
    use terminal::{TerminalProfileId, TerminalStatus as ModelStatus};
    let fixture = Fixture::new().await;
    std::fs::create_dir(fixture._root.path().join("sub")).unwrap();
    std::fs::write(
        fixture._root.path().join("sub/cwd-proof.txt"),
        "LAPIS_ACTUAL_CWD\n",
    )
    .unwrap();
    let first = fixture.config("port-terminal-first").await;
    let second = fixture.config("port-terminal-second").await;
    tokio::task::spawn_blocking(move || {
        let virtual_root = std::env::temp_dir().join("lapis-virtual-terminal-test");
        let first = host::BackendClient::remote(virtual_root.clone(), first).unwrap();
        let second = host::BackendClient::remote(virtual_root.clone(), second).unwrap();
        let terminals = first.terminals();
        let profile = cfg!(windows).then(|| TerminalProfileId::new("cmd"));
        let id = terminals
            .start_profile(&virtual_root.join("sub"), 80, 24, profile.as_ref())
            .unwrap();
        let observer = second.terminals();
        let found = observer
            .list(&virtual_root)
            .unwrap()
            .into_iter()
            .find(|terminal| terminal.id == id)
            .unwrap();
        assert_eq!(found.cwd, virtual_root.join("sub"));
        assert_eq!(found.status, ModelStatus::Running);
        let attached = observer.attach(&virtual_root, &id).unwrap();
        assert_eq!(attached.id, id);
        // 別 Client が操作するには操作権を取得する。
        observer.claim(&id).unwrap();
        drop(terminals);
        let command = if cfg!(windows) {
            "set LAPIS_PART=SHARED\r\necho LAPIS_PORT_%LAPIS_PART%\r\ntype cwd-proof.txt\r\n"
        } else {
            "printf 'LAPIS_PORT_%s\\n' SHARED\ncat cwd-proof.txt\n"
        };
        observer
            .input(&id, &TerminalInput::Text(command.to_owned()))
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            let snapshot = observer
                .list(&virtual_root)
                .unwrap()
                .into_iter()
                .find(|terminal| terminal.id == id)
                .unwrap();
            let output = snapshot.screen.plain_text();
            let succeeded =
                output.contains("LAPIS_PORT_SHARED") && output.contains("LAPIS_ACTUAL_CWD");
            if succeeded {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "shell did not execute: {output}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        observer.terminate(&id).unwrap();
        assert_eq!(
            observer
                .list(&virtual_root)
                .unwrap()
                .into_iter()
                .find(|terminal| terminal.id == id)
                .unwrap()
                .status,
            ModelStatus::Exited
        );
    })
    .await
    .unwrap();
}

fn hello(id: &str) -> HelloRequest {
    HelloRequest {
        protocol: ProtocolRange::exact(CURRENT_PROTOCOL),
        client_id: ClientId::try_new(id).unwrap(),
        client_name: id.to_owned(),
        client_kind: ClientKind::Desktop,
        requested_capabilities: host::supported_capabilities(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn desktop_connection_without_workspace_uses_host_selection() {
    let fixture = Fixture::new().await;
    let mut config = fixture.config("desktop-auto-workspace").await;
    assert!(
        config
            .hello
            .requested_capabilities
            .iter()
            .any(|capability| capability.as_str() == "workspace.switch")
    );
    config.workspace_id = None;
    let expected = fixture.workspace.clone();
    let root = fixture._root.path().to_owned();
    tokio::task::spawn_blocking(move || {
        let connection = QuicConnection::connect(config.clone()).unwrap();
        assert_eq!(connection.workspace_id(), &expected);
        let backend = host::BackendClient::remote(root.clone(), config).unwrap();
        assert_eq!(backend.register_workspace(&root).unwrap(), expected);
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn desktop_workbench_port_receives_remote_updates() {
    use workbench::{ConversationViewState, WorkbenchUpdate};
    let fixture = Fixture::new().await;
    let first = fixture.config("workbench-port-first").await;
    let second = fixture.config("workbench-port-second").await;
    tokio::task::spawn_blocking(move || {
        let root = std::env::temp_dir().join("lapis-virtual-workbench-test");
        let first = host::BackendClient::remote(root.clone(), first).unwrap();
        let second = host::BackendClient::remote(root.clone(), second).unwrap();
        let workspace =
            editor_core::WorkspaceId::new(first.register_workspace(&root).unwrap().as_str());
        let writer = first.workbench();
        let reader = second.workbench();
        let mut subscription = reader.subscribe(&workspace).unwrap();
        let initial = reader.get(&workspace).unwrap();
        let view = ConversationViewState {
            show_panel_controls: false,
            ..Default::default()
        };
        let WorkbenchUpdate::Updated(updated) = writer
            .update(&workspace, initial.revision, view.clone())
            .unwrap()
        else {
            panic!("update")
        };
        assert_eq!(
            subscription.recv_timeout(Duration::from_secs(5)).unwrap(),
            Some(updated.revision)
        );
        assert_eq!(reader.get(&workspace).unwrap().view, Some(view));
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_session_switches_workspace_in_place_after_close() {
    let fixture = Fixture::new().await;
    let other_root = tempfile::tempdir().unwrap();
    let other = fixture.host.register_workspace(other_root.path()).unwrap();
    let token = fixture
        .auth
        .lock()
        .unwrap()
        .begin_user_session_pairing(fixture.workspace.clone())
        .unwrap();
    let mut client = fixture.connect().await;
    client
        .send(&ClientMessage::Pair(PairRequest::new(
            hello("switch-in-place"),
            token.secret_bytes(),
        )))
        .await
        .unwrap();
    let ServerMessage::Paired(mut paired) = receive(&mut client).await else {
        panic!("user pairing")
    };
    let credential = paired.credential_id().unwrap();
    let secret = zeroize::Zeroizing::new(paired.take_secret().unwrap());
    drop(client);

    let mut client = fixture.connect().await;
    client
        .send(&ClientMessage::Authenticate(AuthenticateRequest::new(
            hello("switch-in-place"),
            fixture.workspace.clone(),
            &credential,
            &secret,
        )))
        .await
        .unwrap();
    assert!(matches!(
        receive(&mut client).await,
        ServerMessage::Authenticated(_)
    ));
    connect_workspace(&mut client, &fixture.workspace).await;

    // close を挟まずに別 Workspace へは移れない。
    assert!(matches!(
        request(
            &mut client,
            2,
            RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
                workspace_id: other.clone(),
            })
        )
        .await,
        ResponseBody::Error(ref error) if error.code.as_str() == FORBIDDEN
    ));

    // close しても Session は生きており、同じ接続のまま切替先へ移れる。
    assert!(matches!(
        request(
            &mut client,
            3,
            RequestBody::WorkspaceClose(WorkspaceCloseRequest {
                workspace_id: fixture.workspace.clone(),
            })
        )
        .await,
        ResponseBody::WorkspaceClose(_)
    ));
    assert!(matches!(
        request(
            &mut client,
            4,
            RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
                workspace_id: other.clone(),
            })
        )
        .await,
        ResponseBody::WorkspaceConnect(_)
    ));
    // 切替後は新しい Workspace の操作だけが通る。
    assert!(matches!(
        request(
            &mut client,
            5,
            RequestBody::SnapshotResync(SnapshotRequest {
                workspace_id: other.clone(),
                reason: SnapshotReason::Reconnect,
            })
        )
        .await,
        ResponseBody::SnapshotResync(_)
    ));
    assert!(matches!(
        request(
            &mut client,
            6,
            RequestBody::WorkbenchGet(WorkbenchGetRequest {
                workspace_id: fixture.workspace.clone(),
            })
        )
        .await,
        ResponseBody::Error(ref error) if error.code.as_str() == FORBIDDEN
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workbench_updates_and_conflicts_cross_authenticated_quic_sessions() {
    let fixture = Fixture::new().await;
    let (mut first, _) = fixture.pair("workbench-first").await;
    let (mut second, _) = fixture.pair("workbench-second").await;
    connect_workspace(&mut first, &fixture.workspace).await;
    connect_workspace(&mut second, &fixture.workspace).await;
    let ResponseBody::WorkbenchGet(initial) = request(
        &mut first,
        2,
        RequestBody::WorkbenchGet(WorkbenchGetRequest {
            workspace_id: fixture.workspace.clone(),
        }),
    )
    .await
    else {
        panic!("initial workbench")
    };
    assert!(initial.view.is_null());
    let put = WorkbenchPutRequest {
        workspace_id: fixture.workspace.clone(),
        expected_revision: initial.revision,
        view: serde_json::json!({"show_panel_controls": false}),
    };
    let ResponseBody::WorkbenchPut(WorkbenchPutResponse::Updated(updated)) =
        request(&mut first, 3, RequestBody::WorkbenchPut(put.clone())).await
    else {
        panic!("workbench update")
    };
    let ServerMessage::Event(event) = receive(&mut second).await else {
        panic!("workbench event")
    };
    assert!(
        matches!(event.body, EventBody::WorkbenchChanged { workspace_id, revision } if workspace_id == fixture.workspace && revision == updated.revision)
    );
    let ResponseBody::WorkbenchPut(WorkbenchPutResponse::Conflict(current)) =
        request(&mut second, 2, RequestBody::WorkbenchPut(put)).await
    else {
        panic!("stale write")
    };
    assert_eq!(current, updated);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workbench_write_is_rejected_for_read_only_grant() {
    let fixture = Fixture::new().await;
    let mut client = fixture.connect().await;
    let token = fixture
        .auth
        .lock()
        .unwrap()
        .begin_pairing(fixture.workspace.clone())
        .unwrap();
    let mut identity = hello("workbench-read-only");
    identity.requested_capabilities = CapabilitySet::try_new([
        CapabilityId::try_new(capability::WORKSPACES_CONNECT).unwrap(),
        CapabilityId::try_new(capability::WORKSPACE_SYNC).unwrap(),
    ])
    .unwrap();
    client
        .send(&ClientMessage::Pair(PairRequest::new(
            identity,
            token.secret_bytes(),
        )))
        .await
        .unwrap();
    assert!(matches!(
        receive(&mut client).await,
        ServerMessage::Paired(_)
    ));
    connect_workspace(&mut client, &fixture.workspace).await;
    let ResponseBody::WorkbenchGet(initial) = request(
        &mut client,
        2,
        RequestBody::WorkbenchGet(WorkbenchGetRequest {
            workspace_id: fixture.workspace.clone(),
        }),
    )
    .await
    else {
        panic!("workbench read")
    };
    let result = request(
        &mut client,
        3,
        RequestBody::WorkbenchPut(WorkbenchPutRequest {
            workspace_id: fixture.workspace.clone(),
            expected_revision: initial.revision,
            view: serde_json::json!({}),
        }),
    )
    .await;
    assert!(matches!(result, ResponseBody::Error(error) if error.code.as_str() == FORBIDDEN));
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_user_session_lists_and_connects_registered_workspaces_without_management() {
    let fixture = Fixture::new().await;
    let other_root = tempfile::tempdir().unwrap();
    let other = fixture.host.register_workspace(other_root.path()).unwrap();
    let (mut scoped, _) = fixture.pair("scoped-client").await;
    let ResponseBody::WorkspaceList(scoped_list) = request(
        &mut scoped,
        1,
        RequestBody::WorkspaceList(WorkspaceListRequest::default()),
    )
    .await
    else {
        panic!("scoped list")
    };
    assert_eq!(scoped_list.workspaces.len(), 1);
    assert_eq!(scoped_list.workspaces[0].workspace_id, fixture.workspace);

    let token = fixture
        .auth
        .lock()
        .unwrap()
        .begin_user_session_pairing(fixture.workspace.clone())
        .unwrap();
    let mut first = fixture.connect().await;
    first
        .send(&ClientMessage::Pair(PairRequest::new(
            hello("user-client"),
            token.secret_bytes(),
        )))
        .await
        .unwrap();
    let ServerMessage::Paired(mut paired) = receive(&mut first).await else {
        panic!("user pairing")
    };
    let switch = CapabilityId::try_new(capability::WORKSPACES_SWITCH).unwrap();
    let manage = CapabilityId::try_new(capability::HOST_MANAGE).unwrap();
    assert!(paired.hello.granted_capabilities.contains(&switch));
    assert!(!paired.hello.granted_capabilities.contains(&manage));
    let credential = paired.credential_id().unwrap();
    let secret = zeroize::Zeroizing::new(paired.take_secret().unwrap());
    let ResponseBody::WorkspaceList(list) = request(
        &mut first,
        1,
        RequestBody::WorkspaceList(WorkspaceListRequest::default()),
    )
    .await
    else {
        panic!("user list")
    };
    assert_eq!(list.workspaces.len(), 2);
    assert!(
        list.workspaces
            .iter()
            .any(|workspace| workspace.workspace_id == other)
    );
    let mut second = fixture.connect().await;
    second
        .send(&ClientMessage::Authenticate(AuthenticateRequest::new(
            hello("user-client"),
            other.clone(),
            &credential,
            &secret,
        )))
        .await
        .unwrap();
    assert!(matches!(
        receive(&mut second).await,
        ServerMessage::Authenticated(_)
    ));
    assert!(matches!(
        request(
            &mut second,
            1,
            RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
                workspace_id: other
            })
        )
        .await,
        ResponseBody::WorkspaceConnect(_)
    ));
    assert!(matches!(
        request(
            &mut second,
            2,
            RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
                workspace_id: fixture.workspace.clone()
            })
        )
        .await,
        ResponseBody::Error(_)
    ));
    let third_root = tempfile::tempdir().unwrap();
    assert!(matches!(
        request(
            &mut first,
            2,
            RequestBody::HostRegister(HostRegisterRequest {
                root: third_root.path().to_string_lossy().into_owned()
            })
        )
        .await,
        ResponseBody::HostRegister(_)
    ));
    assert!(matches!(
        request(&mut first, 3, RequestBody::HostWorktree(editor_core::TaskId::new("untrusted"))).await,
        ResponseBody::Error(ref error) if error.code.as_str() == FORBIDDEN
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_session_without_workspace_uses_the_host_primary() {
    let fixture = Fixture::new().await;
    let other_root = tempfile::tempdir().unwrap();
    let other = fixture.host.register_workspace(other_root.path()).unwrap();
    let token = fixture
        .auth
        .lock()
        .unwrap()
        .begin_user_session_pairing(other)
        .unwrap();
    let mut paired = fixture.connect().await;
    paired
        .send(&ClientMessage::Pair(PairRequest::new(
            hello("primary-user"),
            token.secret_bytes(),
        )))
        .await
        .unwrap();
    let ServerMessage::Paired(mut response) = receive(&mut paired).await else {
        panic!("user pairing")
    };
    let credential = response.credential_id().unwrap();
    let secret = zeroize::Zeroizing::new(response.take_secret().unwrap());
    drop(paired);

    let mut client = fixture.connect().await;
    client
        .send(&ClientMessage::Authenticate(AuthenticateRequest::new_auto(
            hello("primary-user"),
            &credential,
            &secret,
        )))
        .await
        .unwrap();
    let ServerMessage::Authenticated(hello) = receive(&mut client).await else {
        panic!("auto authentication")
    };
    assert_eq!(hello.workspace_id, Some(fixture.workspace.clone()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn common_async_client_sends_requests_to_real_host() {
    let fixture = Fixture::new().await;
    let config = fixture.config("async-client").await;
    let connection = tokio::task::spawn_blocking(move || QuicConnection::connect(config))
        .await
        .unwrap()
        .unwrap();
    let result = AsyncHostConnection::request(
        &connection,
        RequestBody::SnapshotResync(SnapshotRequest {
            workspace_id: fixture.workspace.clone(),
            reason: SnapshotReason::Initial,
        }),
    )
    .await
    .unwrap();
    assert!(matches!(result, ResponseBody::SnapshotResync(_)));
    let mut sync = async_workbench::AsyncWorkbenchSync::new(&connection);
    let initial = sync.open(fixture.workspace.clone()).await.unwrap();
    let view = workbench::ConversationViewState {
        show_panel_controls: false,
        ..Default::default()
    };
    let updated = async_workbench::update(&connection, &fixture.workspace, initial.revision, &view)
        .await
        .unwrap();
    assert!(matches!(updated, workbench::WorkbenchUpdate::Updated(_)));
    let local = workbench::ConversationViewState::default();
    let conflict = sync.save(local.clone()).await.unwrap();
    assert!(matches!(conflict, workbench::WorkbenchUpdate::Conflict(_)));
    assert_eq!(sync.pending(), Some(&local));
    assert!(matches!(
        sync.retry_local().await.unwrap(),
        workbench::WorkbenchUpdate::Updated(_)
    ));
    assert!(sync.pending().is_none());
    let connection = connection.clone();
    let workspace = fixture.workspace.clone();
    let response = tokio::task::spawn_blocking(move || {
        std::thread::spawn(move || {
            use std::{
                future::Future,
                task::{Context, Poll, Wake, Waker},
            };
            struct ThreadWake(std::thread::Thread);
            impl Wake for ThreadWake {
                fn wake(self: Arc<Self>) {
                    self.0.unpark();
                }
                fn wake_by_ref(self: &Arc<Self>) {
                    self.0.unpark();
                }
            }
            let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
            let mut context = Context::from_waker(&waker);
            let request = AsyncHostConnection::request(
                &connection,
                RequestBody::SnapshotResync(SnapshotRequest {
                    workspace_id: workspace,
                    reason: SnapshotReason::Initial,
                }),
            );
            let mut request = std::pin::pin!(request);
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            loop {
                match request.as_mut().poll(&mut context) {
                    Poll::Ready(result) => break result,
                    Poll::Pending => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "async request timed out"
                        );
                        std::thread::park_timeout(Duration::from_millis(100));
                    }
                }
            }
        })
        .join()
        .unwrap()
    })
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(response, ResponseBody::SnapshotResync(_)));
}

#[test]
fn quic_connection_recovers_attachments_without_replaying_successful_edits() {
    run(async {
        let fixture = Fixture::new().await;
        let connection = QuicConnection::connect(fixture.config("sync-client").await).unwrap();
        let receiver = connection.subscribe().unwrap();
        let ResponseBody::DocumentCreate(created) = connection
            .request(RequestBody::DocumentCreate(DocumentCreateRequest {
                workspace_id: fixture.workspace.clone(),
                path: WorkspaceRelativePath::parse("reconnect.ts").unwrap(),
                encoding: DocumentEncoding::Utf8,
                content: "let value = 1;".to_owned(),
            }))
            .unwrap()
        else {
            panic!("create failed")
        };
        let ResponseBody::DocumentEdit(edited) = connection
            .request(RequestBody::DocumentEdit(DocumentEditRequest {
                document_id: created.document.document_id.clone(),
                base_revision: created.document.revision,
                transaction: DocumentTransaction::try_new(vec![
                    DocumentTextEdit::try_new(12, 13, "42").unwrap(),
                ])
                .unwrap(),
            }))
            .unwrap()
        else {
            panic!("edit failed")
        };
        connection.disconnect().unwrap();
        loop {
            if receiver.recv_timeout(Duration::from_secs(2)).is_err() {
                break;
            }
        }
        let ResponseBody::SnapshotResync(response) = connection
            .request(RequestBody::SnapshotResync(SnapshotRequest {
                workspace_id: fixture.workspace.clone(),
                reason: SnapshotReason::Reconnect,
            }))
            .unwrap()
        else {
            panic!("resync failed")
        };
        let document = response
            .snapshot
            .documents
            .iter()
            .find(|document| document.document_id == created.document.document_id)
            .unwrap();
        assert_eq!(document.content, "let value = 42;");
        assert_eq!(document.revision, edited.revision);
        assert!(document.dirty);
    });
}

#[test]
fn remote_desktop_ports_edit_and_save_without_accessing_client_filesystem() {
    run(async {
        let fixture = Fixture::new().await;
        std::fs::write(fixture._root.path().join("remote.ts"), "let value = 1;").unwrap();
        let namespace = fixture._root.path().join("nonexistent-client-namespace");
        let backend =
            host::BackendClient::remote(namespace.clone(), fixture.config("desktop-port").await)
                .unwrap();
        assert!(!namespace.exists());
        assert!(
            backend
                .workspaces()
                .list_tree(&namespace)
                .unwrap()
                .iter()
                .any(|file| file.path == namespace.join("remote.ts"))
        );
        let documents = backend.documents();
        let opened = documents.open(&namespace.join("remote.ts")).unwrap();
        let edited = documents
            .edit(&opened.id, opened.document.revision(), 12..13, "42")
            .unwrap();
        assert!(
            documents
                .edit(&opened.id, opened.document.revision(), 0..0, "stale")
                .is_err()
        );
        let saved = documents
            .save(&opened.id, edited.document.revision(), None)
            .unwrap();
        assert!(!saved.document.is_dirty());
        assert_eq!(
            std::fs::read_to_string(fixture._root.path().join("remote.ts")).unwrap(),
            "let value = 42;"
        );
        assert!(!namespace.exists());
        let draft = documents.create(Some(&namespace)).unwrap();
        let draft = documents
            .edit(
                &draft.id,
                draft.document.revision(),
                0..0,
                "const remote = true;",
            )
            .unwrap();
        let saved_as = documents
            .save(
                &draft.id,
                draft.document.revision(),
                Some(namespace.join("created.ts")),
            )
            .unwrap();
        assert_eq!(saved_as.id, draft.id);
        assert!(!saved_as.document.is_dirty());
        assert_eq!(
            std::fs::read_to_string(fixture._root.path().join("created.ts")).unwrap(),
            "const remote = true;"
        );
        let dirty = documents
            .edit(
                &saved_as.id,
                saved_as.document.revision(),
                0..0,
                "// local draft\n",
            )
            .unwrap();
        std::fs::write(fixture._root.path().join("created.ts"), "external change").unwrap();
        assert!(
            documents
                .save(&dirty.id, dirty.document.revision(), None)
                .is_err()
        );
        let retained = documents.attach(&dirty.id).unwrap();
        assert!(retained.document.is_dirty());
        assert!(
            retained
                .document
                .content_for_save()
                .starts_with("// local draft")
        );
        assert_eq!(
            std::fs::read_to_string(fixture._root.path().join("created.ts")).unwrap(),
            "external change"
        );
        assert!(!namespace.exists());
        assert!(
            documents
                .open(&fixture._root.path().join("remote.ts"))
                .is_err(),
            "Client paths outside the Remote namespace must not be sent to Host"
        );
    });
}

#[test]
fn normal_pairing_cannot_register_folders_or_transfer_documents() {
    run(async {
        let fixture = Fixture::new().await;
        let (mut client, _) = fixture.pair("guest").await;
        connect_workspace(&mut client, &fixture.workspace).await;
        let ResponseBody::Error(denied) = request(
            &mut client,
            2,
            RequestBody::HostRegister(HostRegisterRequest {
                root: fixture._root.path().to_string_lossy().into_owned(),
            }),
        )
        .await
        else {
            panic!("guest must not manage Host")
        };
        assert_eq!(denied.code.as_str(), FORBIDDEN);
        let ResponseBody::Error(denied) = request(
            &mut client,
            3,
            RequestBody::DocumentRelocate(DocumentRelocateRequest {
                document_id: DocumentId::try_new("unknown").unwrap(),
                destination_workspace: fixture.workspace.clone(),
                base_revision: Revision::new(1),
                path: WorkspaceRelativePath::parse("new.ts").unwrap(),
            }),
        )
        .await
        else {
            panic!("guest must not transfer documents")
        };
        assert_eq!(denied.code.as_str(), FORBIDDEN);
        let ResponseBody::Error(denied) = request(
            &mut client,
            4,
            RequestBody::HostWorktree(editor_core::TaskId::new("unknown")),
        )
        .await
        else {
            panic!("guest must not resolve Host filesystem paths")
        };
        assert_eq!(denied.code.as_str(), FORBIDDEN);
    });
}

#[test]
fn invitation_import_is_one_time_and_protected_profile_restores_the_same_host_documents() {
    run(async {
        let fixture = Fixture::new().await;
        let state = tempfile::tempdir().unwrap();
        std::fs::write(fixture._root.path().join("profile.ts"), "let value = 1;").unwrap();
        let token = fixture
            .auth
            .lock()
            .unwrap()
            .begin_pairing(fixture.workspace.clone())
            .unwrap();
        let path = state.path().join("invite.json");
        let invitation = host::HostInvitation {
            version: 1,
            address: fixture.address(),
            server_name: "localhost".to_owned(),
            certificate: fixture.cert.to_vec(),
            workspace_id: fixture.workspace.as_str().to_owned(),
            name: "RemoteProject".to_owned(),
            token: token.secret_bytes().to_vec(),
        };
        host::write_invitation(&path, &invitation).unwrap();
        assert!(host::write_invitation(&path, &invitation).is_err());
        let profiles = state.path().join("profiles");
        let (backend, namespace, profile) = {
            let path = path.clone();
            let profiles = profiles.clone();
            tokio::task::spawn_blocking(move || host::open_host_file(&path, &profiles))
                .await
                .unwrap()
                .unwrap()
        };
        let documents = backend.documents();
        let original = documents.open(&namespace.join("profile.ts")).unwrap();
        let edited = documents
            .edit(&original.id, original.document.revision(), 12..13, "42")
            .unwrap();
        assert!(edited.document.is_dirty());
        drop(documents);
        drop(backend);
        let (restored, restored_namespace, _) = {
            let profile = profile.clone();
            let profiles = profiles.clone();
            tokio::task::spawn_blocking(move || host::open_host_file(&profile, &profiles))
                .await
                .unwrap()
                .unwrap()
        };
        assert_eq!(namespace, restored_namespace);
        let document = restored
            .documents()
            .open(&namespace.join("profile.ts"))
            .unwrap();
        assert_eq!(document.id, original.id);
        assert_eq!(document.document.content_for_save(), "let value = 42;");
        let before = zeroize::Zeroizing::new(std::fs::read(&profile).unwrap());
        #[cfg(windows)]
        assert_ne!(before.first(), Some(&b'{'));
        let reused = {
            let path = path.clone();
            let profiles = profiles.clone();
            tokio::task::spawn_blocking(move || host::open_host_file(&path, &profiles))
                .await
                .unwrap()
        };
        // 使用済みの招待は新しいcredentialを作らない。同じHostのprofileがあればそれを再利用する。
        let (reused_backend, reused_namespace, reused_profile) =
            reused.expect("同じHostへ接続済みなら、1回限りの招待を使い切っていても接続できる");
        assert_eq!(reused_namespace, namespace);
        assert_eq!(reused_profile, profile);
        drop(reused_backend);
        // 使用済みの招待を、別のHost（証明書が違う）の流用には使わない。
        let other = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        let other_invitation = host::HostInvitation {
            version: 1,
            address: fixture.address(),
            server_name: "localhost".to_owned(),
            certificate: other.cert.der().to_vec(),
            workspace_id: fixture.workspace.as_str().to_owned(),
            name: "RemoteProject".to_owned(),
            token: token.secret_bytes().to_vec(),
        };
        let other_path = state.path().join("other.json");
        host::write_invitation(&other_path, &other_invitation).unwrap();
        let other_result = {
            let profiles = profiles.clone();
            tokio::task::spawn_blocking(move || host::open_host_file(&other_path, &profiles))
                .await
                .unwrap()
        };
        assert!(
            other_result.is_err(),
            "別のHostを指す使用済み招待を既存profileへ流用してはいけない"
        );
        assert!(std::fs::read(&profile).unwrap().as_slice() == before.as_slice());
        std::fs::write(&profile, b"corrupt data").unwrap();
        assert!(host::open_host_file(&profile, &profiles).is_err());
    });
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
                profile: None,
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
                3,
                RequestBody::TerminalClaim(TerminalClaimRequest {
                    terminal_id: terminal.clone(),
                })
            )
            .await,
            ResponseBody::TerminalClaim(_)
        ));
        let input_response = request(
            &mut second,
            2,
            RequestBody::TerminalInput(TerminalInputRequest {
                terminal_id: terminal.clone(),
                input: TerminalInput::Text(
                    if cfg!(windows) {
                        "Write-Output ('LAPIS_QUIC_' + 'SHARED_PTY')\r\n"
                    } else {
                        "printf 'LAPIS_QUIC_%s\\n' SHARED_PTY\n"
                    }
                    .to_owned(),
                ),
            }),
        )
        .await;
        assert!(
            matches!(input_response, ResponseBody::TerminalInput(_)),
            "terminal input failed: {input_response:?}"
        );
        let mut last_output = String::new();
        let completed = tokio::time::timeout(Duration::from_secs(15), async {
            let mut number = 4;
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
                last_output = value.screen.plain_text();
                if last_output.contains("LAPIS_QUIC_SHARED_PTY") {
                    break;
                }
                number += 1;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        assert!(
            completed.is_ok(),
            "shell output missing: {:?}",
            &last_output[last_output.len().saturating_sub(4096)..]
        );
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
