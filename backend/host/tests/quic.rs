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
    async fn config(&self, id: &str) -> QuicConnectionConfig {
        self.config_for_sources(id, Vec::new()).await
    }
    async fn config_for_sources(
        &self,
        id: &str,
        sources: Vec<WorkspaceId>,
    ) -> QuicConnectionConfig {
        let (client, mut paired) = self.pair_project(id, sources).await;
        drop(client);
        QuicConnectionConfig {
            address: self.address(),
            server_name: "localhost".to_owned(),
            certificate: self.cert.clone(),
            hello: hello(id),
            workspace_id: self.workspace.clone(),
            credential: Arc::new(CredentialHandle::from_parts(
                paired.credential_id().unwrap(),
                &mut paired.take_secret().unwrap(),
            )),
        }
    }
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
            Arc::new(lapis_host::HostRemoteHandler::new(host.clone()).with_auth(auth.clone())),
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
        self.pair_project(id, Vec::new()).await
    }
    async fn pair_project(
        &self,
        id: &str,
        sources: Vec<WorkspaceId>,
    ) -> (QuicClient, PairedResponse) {
        let mut client = self.connect().await;
        let token = self
            .auth
            .lock()
            .unwrap()
            .begin_project_pairing(self.workspace.clone(), sources)
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
async fn remote_project_catalog_exposes_only_owner_selected_sources() {
    let fixture = Fixture::new().await;
    let sub = tempfile::tempdir().unwrap();
    let unrelated = tempfile::tempdir().unwrap();
    std::fs::write(fixture._root.path().join("note.ts"), "primary").unwrap();
    std::fs::write(sub.path().join("note.ts"), "sub").unwrap();
    let sub_id = fixture.host.register_workspace(sub.path()).unwrap();
    let unrelated_id = fixture.host.register_workspace(unrelated.path()).unwrap();
    let config = fixture
        .config_for_sources("project-sources", vec![sub_id.clone()])
        .await;
    tokio::task::spawn_blocking(move || {
        let virtual_root = unrelated.path().join("virtual-source-namespace");
        let backend =
            lapis_host::BackendClient::remote(virtual_root.clone(), config.clone()).unwrap();
        let catalog = backend.workspaces().available_sources().unwrap();
        assert_eq!(catalog.len(), 2);
        let alias = catalog
            .iter()
            .find(|(root, _)| root != &virtual_root)
            .unwrap()
            .0
            .clone();
        assert!(alias.starts_with(&virtual_root));
        assert!(!alias.exists());
        assert_eq!(backend.register_workspace(&alias).unwrap(), sub_id);
        let documents = backend.documents();
        let primary = documents.open(&virtual_root.join("note.ts")).unwrap();
        let secondary = documents.open(&alias.join("note.ts")).unwrap();
        assert_ne!(primary.id, secondary.id);
        assert_eq!(primary.document.content_for_save(), "primary");
        assert_eq!(secondary.document.content_for_save(), "sub");
        let draft = documents
            .edit(
                &secondary.id,
                secondary.document.revision(),
                0..3,
                "sub draft",
            )
            .unwrap();
        let fresh =
            lapis_host::BackendClient::remote(virtual_root.clone(), config.clone()).unwrap();
        assert_eq!(fresh.register_workspace(&alias).unwrap(), sub_id);
        let restored = fresh.documents().open(&alias.join("note.ts")).unwrap();
        assert_eq!(restored.id, draft.id);
        assert_eq!(restored.document.content_for_save(), "sub draft");
        let outside = virtual_root
            .join(".lapis-sources")
            .join(unrelated_id.as_str());
        assert!(backend.register_workspace(&outside).is_err());
        let mut forbidden = config;
        forbidden.workspace_id = unrelated_id;
        assert!(QuicConnection::connect(forbidden).is_err());
        assert_eq!(
            std::fs::read_to_string(sub.path().join("note.ts")).unwrap(),
            "sub"
        );
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_worktree_ports_use_virtual_paths_and_restore_scoped_bindings() {
    let fixture = Fixture::new().await;
    let sibling = tempfile::tempdir().unwrap();
    let worktree_path = sibling.path().join("topic");
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .current_dir(fixture._root.path())
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.name", "Lapis Test"]);
    git(&["config", "user.email", "test@example.invalid"]);
    git(&["config", "core.autocrlf", "false"]);
    git(&[
        "config",
        "core.hooksPath",
        sibling.path().join("no-hooks").to_str().unwrap(),
    ]);
    std::fs::write(fixture._root.path().join("note.ts"), "base").unwrap();
    git(&["add", "note.ts"]);
    git(&["commit", "-m", "initial"]);
    git(&[
        "worktree",
        "add",
        "-b",
        "topic",
        worktree_path.to_str().unwrap(),
    ]);
    let config = fixture.config("remote-worktree").await;
    let auth = fixture.auth.clone();
    let host = fixture.host.clone();
    let root_id = fixture.workspace.clone();
    let original = fixture._root.path().to_owned();
    tokio::task::spawn_blocking(move || {
        let virtual_root = sibling.path().join("client-namespace");
        let backend =
            lapis_host::BackendClient::remote(virtual_root.clone(), config.clone()).unwrap();
        let source = lapis_editor_core::WorkspaceId::new(root_id.as_str());
        let git = backend.git();
        let trees = git.worktrees(&source).unwrap();
        assert_eq!(trees.len(), 1);
        let tree = &trees[0].worktree_id;
        assert!(
            git.open_worktree(&source, &lapis_editor_core::TaskId::new("foreign-worktree"))
                .is_err()
        );
        let mut ungranted = config.clone();
        ungranted.workspace_id = host.register_workspace(sibling.path()).unwrap();
        assert!(QuicConnection::connect(ungranted).is_err());
        let alias = git.open_worktree(&source, tree).unwrap();
        assert!(alias.starts_with(&virtual_root));
        assert!(!alias.exists());
        assert_ne!(alias, worktree_path);
        let documents = backend.documents();
        let opened = documents.open(&alias.join("note.ts")).unwrap();
        let edited = documents
            .edit(&opened.id, opened.document.revision(), 0..4, "remote edit")
            .unwrap();
        documents
            .save(&edited.id, edited.document.revision(), None)
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(worktree_path.join("note.ts")).unwrap(),
            "remote edit"
        );
        assert_eq!(
            std::fs::read_to_string(original.join("note.ts")).unwrap(),
            "base"
        );
        let terminals = backend.terminals();
        let terminal = terminals.start(&alias, 80, 24).unwrap();
        terminals
            .input(
                &terminal,
                if cfg!(windows) {
                    b"Write-Output ('REMOTE_TREE_' + 'ALIVE')\r"
                } else {
                    b"printf 'REMOTE_TREE_%s\\n' ALIVE\r"
                },
            )
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut output = Vec::new();
        loop {
            for event in terminals.poll(&terminal).unwrap() {
                if let lapis_terminal::TerminalEvent::Output(data) = event {
                    output.extend(data.data);
                }
            }
            if output
                .windows(b"REMOTE_TREE_ALIVE".len())
                .any(|bytes| bytes == b"REMOTE_TREE_ALIVE")
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Remote Worktree PTY did not respond"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        terminals.terminate(&terminal).unwrap();
        let fresh = lapis_host::BackendClient::remote(virtual_root, config.clone()).unwrap();
        let id = fresh.register_workspace(&alias).unwrap();
        assert_ne!(id, root_id);
        let restored = fresh.documents().open(&alias.join("note.ts")).unwrap();
        assert_eq!(restored.id, opened.id);
        assert_eq!(restored.document.content_for_save(), "remote edit");
        auth.lock().unwrap().revoke(config.credential.id());
        assert!(fresh.documents().open(&alias.join("note.ts")).is_err());
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn desktop_ports_discover_and_reattach_the_same_live_terminal_with_its_cwd() {
    use lapis_terminal::{TerminalShell, TerminalStatus as ModelStatus};
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
        let first = lapis_host::BackendClient::remote(virtual_root.clone(), first).unwrap();
        let second = lapis_host::BackendClient::remote(virtual_root.clone(), second).unwrap();
        let terminals = first.terminals();
        let shell = if cfg!(windows) {
            TerminalShell::Cmd
        } else {
            TerminalShell::Default
        };
        let id = terminals
            .start_with_shell(&virtual_root.join("sub"), 80, 24, shell)
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
        drop(terminals);
        let command = if cfg!(windows) {
            "set LAPIS_PART=SHARED\r\necho LAPIS_PORT_%LAPIS_PART%\r\ntype cwd-proof.txt\r\n"
        } else {
            "printf 'LAPIS_PORT_%s\\n' SHARED\ncat cwd-proof.txt\n"
        };
        observer.input(&id, command.as_bytes()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            let snapshot = observer
                .list(&virtual_root)
                .unwrap()
                .into_iter()
                .find(|terminal| terminal.id == id)
                .unwrap();
            let output = String::from_utf8_lossy(&snapshot.output);
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
        let backend = lapis_host::BackendClient::remote(
            namespace.clone(),
            fixture.config("desktop-port").await,
        )
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
        let invitation = lapis_host::HostInvitation {
            version: 1,
            address: fixture.address(),
            server_name: "localhost".to_owned(),
            certificate: fixture.cert.to_vec(),
            workspace_id: fixture.workspace.as_str().to_owned(),
            name: "RemoteProject".to_owned(),
            token: token.secret_bytes().to_vec(),
        };
        lapis_host::write_invitation(&path, &invitation).unwrap();
        assert!(lapis_host::write_invitation(&path, &invitation).is_err());
        let profiles = state.path().join("profiles");
        let (backend, namespace, profile) = {
            let path = path.clone();
            let profiles = profiles.clone();
            tokio::task::spawn_blocking(move || lapis_host::open_host_file(&path, &profiles))
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
            tokio::task::spawn_blocking(move || lapis_host::open_host_file(&profile, &profiles))
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
            tokio::task::spawn_blocking(move || lapis_host::open_host_file(&path, &profiles))
                .await
                .unwrap()
        };
        assert!(
            reused.is_err(),
            "used invitations must not silently replace a credential"
        );
        assert!(std::fs::read(&profile).unwrap().as_slice() == before.as_slice());
        std::fs::write(&profile, b"corrupt data").unwrap();
        assert!(lapis_host::open_host_file(&profile, &profiles).is_err());
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
                shell: Default::default(),
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
        let mut last_output = Vec::new();
        let completed = tokio::time::timeout(Duration::from_secs(15), async {
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
                last_output = value.buffered_output.clone();
                if String::from_utf8_lossy(&value.buffered_output).contains("LAPIS_QUIC_SHARED_PTY")
                {
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
            String::from_utf8_lossy(&last_output[last_output.len().saturating_sub(4096)..])
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
