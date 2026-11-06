use lapis_client_api::*;
use lapis_host::{BackendClient, BackendHost};
use lapis_remote::*;
use std::sync::{Arc, Mutex};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn short_lived_clients_release_connections_and_feature_ports_keep_working() {
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
    let token = auth
        .lock()
        .unwrap()
        .begin_pairing(workspace.clone())
        .unwrap();
    let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    let certificate = certified.cert.der().clone();
    let tls = tls13_server_config(
        vec![certificate.clone()],
        rustls::pki_types::PrivateKeyDer::Pkcs8(certified.signing_key.serialize_der().into()),
    )
    .unwrap();
    let server = QuicServer::bind(
        "127.0.0.1:0".parse().unwrap(),
        tls,
        auth,
        Arc::new(BackendRemoteHandler::new(host.service())),
        RemoteLimits::default(),
    )
    .await
    .unwrap();
    let config = QuicConnectionConfig::pair(
        server.listening_address().unwrap(),
        "localhost".to_owned(),
        certificate,
        ClientHello {
            protocol: ProtocolRange::exact(CURRENT_PROTOCOL),
            client_id: ClientId::try_new("lifetime-test").unwrap(),
            client_name: "test".to_owned(),
            client_kind: ClientKind::Desktop,
            requested_capabilities: lapis_host::supported_capabilities(),
        },
        workspace.clone(),
        token.secret_bytes(),
    )
    .await
    .unwrap();
    tokio::task::spawn_blocking(move || {
        let client = BackendClient::remote(
            std::env::temp_dir().join("lapis-lifetime-virtual"),
            config.clone(),
        )
        .unwrap();
        let tasks = client.tasks();
        let model_workspace = lapis_editor_core::WorkspaceId::new(workspace.as_str());
        for _ in 0..160 {
            let connection = QuicConnection::connect(config.clone()).unwrap();
            assert!(matches!(
                connection
                    .request(RequestBody::FileTree(FileTreeRequest {
                        workspace_id: workspace.clone(),
                        path: None,
                    }))
                    .unwrap(),
                ResponseBody::FileTree(_)
            ));
            drop(connection);
            assert!(tasks.load(&model_workspace).unwrap().is_empty());
        }
    })
    .await
    .unwrap();
    server.shutdown().await;
}
