use crate::{
    AuthenticateRequest, ClientMessage, CredentialHandle, PairRequest, QuicClient, RemoteLimits,
    ServerMessage,
};
use backend_state::{BackendEventReceiver, BackendServiceError};
use client_api::*;
use rustls::pki_types::CertificateDer;
use std::{
    net::SocketAddr,
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};
use tokio::time::timeout;
use zeroize::Zeroizing;

type Error = Box<dyn std::error::Error + Send + Sync>;

/// Hostへ提示する認証材料。
#[derive(Clone)]
pub enum QuicAuth {
    Credential(Arc<CredentialHandle>),
    Oidc(Arc<Zeroizing<String>>),
}

#[derive(Clone)]
pub struct QuicConnectionConfig {
    pub address: SocketAddr,
    pub server_name: String,
    pub certificate: CertificateDer<'static>,
    pub hello: HelloRequest,
    /// 接続先 Workspace。None は認証後に Host の選択へ従う。
    pub workspace_id: Option<WorkspaceId>,
    pub auth: QuicAuth,
}

impl QuicConnectionConfig {
    /// 招待はこの一回だけ交換し、再接続には発行済みcredentialを使う。
    pub async fn pair(
        address: SocketAddr,
        server_name: String,
        certificate: CertificateDer<'static>,
        hello: HelloRequest,
        workspace_id: Option<WorkspaceId>,
        token: &[u8; 32],
    ) -> Result<Self, Error> {
        let mut client = QuicClient::connect(
            address,
            &server_name,
            certificate.clone(),
            RemoteLimits::default(),
        )
        .await?;
        client
            .send(&ClientMessage::Pair(PairRequest::new(hello.clone(), token)))
            .await?;
        let message = timeout(Duration::from_secs(10), client.recv()).await??;
        let ServerMessage::Paired(mut paired) = message else {
            return Err("ペアリングが拒否されました".into());
        };
        if paired.hello.protocol != CURRENT_PROTOCOL {
            return Err("Protocolが一致しません".into());
        }
        // token に結び付いた Workspace を Host の応答から採用する。
        let workspace_id = paired.hello.workspace_id.clone().or(workspace_id);
        let credential = Arc::new(CredentialHandle::from_parts(
            paired.credential_id()?,
            &mut paired.take_secret()?,
        ));
        Ok(Self {
            address,
            server_name,
            certificate,
            hello,
            workspace_id,
            auth: QuicAuth::Credential(credential),
        })
    }

    /// 保存済みの OIDC ID Token で再接続する設定を作る。
    pub fn oidc(
        address: SocketAddr,
        server_name: String,
        certificate: CertificateDer<'static>,
        hello: HelloRequest,
        workspace_id: Option<WorkspaceId>,
        id_token: String,
    ) -> Self {
        Self {
            address,
            server_name,
            certificate,
            hello,
            workspace_id,
            auth: QuicAuth::Oidc(Arc::new(Zeroizing::new(id_token))),
        }
    }

    pub fn credential(&self) -> Option<&Arc<CredentialHandle>> {
        match &self.auth {
            QuicAuth::Credential(credential) => Some(credential),
            QuicAuth::Oidc(_) => None,
        }
    }

    pub fn id_token(&self) -> Option<&str> {
        match &self.auth {
            QuicAuth::Credential(_) => None,
            QuicAuth::Oidc(id_token) => Some(id_token.as_str()),
        }
    }

    /// 指定された認証材料で Host に接続し、検証済みの HelloResponse を返す。
    /// 接続は close packet を送ってから破棄する。
    pub async fn verify(&self) -> Result<HelloResponse, Error> {
        let (client, hello) = self.authenticate().await?;
        client.shutdown().await;
        Ok(hello)
    }

    async fn authenticate(&self) -> Result<(QuicClient, HelloResponse), Error> {
        let mut client = QuicClient::connect(
            self.address,
            &self.server_name,
            self.certificate.clone(),
            RemoteLimits::default(),
        )
        .await?;
        let message = match &self.auth {
            QuicAuth::Credential(credential) => {
                ClientMessage::Authenticate(match &self.workspace_id {
                    Some(workspace_id) => AuthenticateRequest::new(
                        self.hello.clone(),
                        workspace_id.clone(),
                        credential.id(),
                        credential.secret_bytes(),
                    ),
                    None => AuthenticateRequest::new_auto(
                        self.hello.clone(),
                        credential.id(),
                        credential.secret_bytes(),
                    ),
                })
            }
            QuicAuth::Oidc(id_token) => ClientMessage::Oidc(match &self.workspace_id {
                Some(workspace_id) => OidcRequest::new(
                    self.hello.clone(),
                    workspace_id.clone(),
                    id_token.to_string(),
                ),
                None => OidcRequest::new_auto(self.hello.clone(), id_token.to_string()),
            }),
        };
        client.send(&message).await?;
        let message = timeout(Duration::from_secs(10), client.recv()).await??;
        match message {
            ServerMessage::Authenticated(hello) if hello.protocol == CURRENT_PROTOCOL => {
                Ok((client, hello))
            }
            ServerMessage::Error(error) => Err(authentication_error(error.code.as_str()).into()),
            _ => Err("Hostの認証が拒否されました".into()),
        }
    }
}

fn authentication_error(code: &str) -> String {
    match code {
        client_api::CREDENTIAL_EXPIRED => {
            "credentialの有効期限が切れています。再ペアリングしてください".to_owned()
        }
        client_api::WORKSPACE_MISMATCH => {
            "指定したWorkspaceは、このcredentialの接続先ではありません".to_owned()
        }
        client_api::WORKSPACE_UNAVAILABLE => {
            "指定したWorkspaceはHostに登録されていません".to_owned()
        }
        client_api::HOST_MISMATCH => "保存しているcredentialは別のHostのものです".to_owned(),
        other => format!("Hostの認証が拒否されました: {other}"),
    }
}

/// 同期portの呼出元はDesktopの機能worker。UI threadから呼ばない。
/// socket・購読の寿命はこのworkerだけが所有し、DropでPTYを終了しない。
#[derive(Clone)]
pub struct QuicConnection {
    inner: Arc<ConnectionInner>,
}
struct ConnectionInner {
    workspace: WorkspaceId,
    sender: tokio::sync::mpsc::Sender<Command>,
}
enum Command {
    Request(RequestBody, std::time::Instant, RequestReply),
    Subscribe(mpsc::SyncSender<BackendEventReceiver>),
    Disconnect(mpsc::SyncSender<()>),
}

enum RequestReply {
    Sync(mpsc::SyncSender<Result<ResponseBody, BackendServiceError>>),
    Async(tokio::sync::oneshot::Sender<Result<ResponseBody, BackendServiceError>>),
}

impl RequestReply {
    fn is_cancelled(&self) -> bool {
        match self {
            Self::Sync(_) => false,
            Self::Async(sender) => sender.is_closed(),
        }
    }

    fn send(self, result: Result<ResponseBody, BackendServiceError>) {
        match self {
            Self::Sync(sender) => {
                let _ = sender.send(result);
            }
            Self::Async(sender) => {
                let _ = sender.send(result);
            }
        }
    }
}

impl QuicConnection {
    pub fn connect(config: QuicConnectionConfig) -> Result<Self, BackendServiceError> {
        let (sender, receiver) = tokio::sync::mpsc::channel(32);
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("lapis-quic-client".to_owned())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(_) => {
                        let _ = started_tx.send(Err(BackendServiceError::Unavailable));
                        return;
                    }
                };
                runtime.block_on(run(config, receiver, started_tx));
            })
            .map_err(BackendServiceError::ThreadSpawn)?;
        match started_rx.recv_timeout(Duration::from_secs(45)) {
            Ok(Ok(workspace)) => Ok(Self {
                inner: Arc::new(ConnectionInner { workspace, sender }),
            }),
            Ok(Err(error)) => Err(error),
            _ => Err(BackendServiceError::Connection(
                "接続開始がtimeoutしました".to_owned(),
            )),
        }
    }
    pub fn workspace_id(&self) -> &WorkspaceId {
        &self.inner.workspace
    }
    pub fn request(&self, body: RequestBody) -> Result<ResponseBody, BackendServiceError> {
        let (sender, receiver) = mpsc::sync_channel(1);
        self.inner
            .sender
            .try_send(Command::Request(
                body,
                std::time::Instant::now() + Duration::from_secs(70),
                RequestReply::Sync(sender),
            ))
            .map_err(|_| BackendServiceError::Unavailable)?;
        receiver
            .recv_timeout(Duration::from_secs(75))
            .map_err(|_| BackendServiceError::Unavailable)?
    }
    pub fn subscribe(&self) -> Result<BackendEventReceiver, BackendServiceError> {
        let (sender, receiver) = mpsc::sync_channel(1);
        self.inner
            .sender
            .try_send(Command::Subscribe(sender))
            .map_err(|_| BackendServiceError::Unavailable)?;
        receiver
            .recv_timeout(Duration::from_secs(75))
            .map_err(|_| BackendServiceError::Unavailable)
    }
    /// 接続だけを落とす。次の操作で同じcredentialを使って再認証する。
    pub fn disconnect(&self) -> Result<(), BackendServiceError> {
        let (sender, receiver) = mpsc::sync_channel(1);
        self.inner
            .sender
            .try_send(Command::Disconnect(sender))
            .map_err(|_| BackendServiceError::Unavailable)?;
        receiver
            .recv_timeout(Duration::from_secs(75))
            .map_err(|_| BackendServiceError::Unavailable)
    }
}

impl client_core::AsyncHostConnection for QuicConnection {
    async fn request(&self, body: RequestBody) -> Result<ResponseBody, String> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.inner
            .sender
            .try_send(Command::Request(
                body,
                std::time::Instant::now() + Duration::from_secs(70),
                RequestReply::Async(sender),
            ))
            .map_err(|_| "Clientの処理待ちが上限に達したか接続が終了しました".to_owned())?;
        receiver
            .await
            .map_err(|_| "Host接続が終了しました".to_owned())?
            .map_err(|error| error.to_string())
    }
}

async fn run(
    mut config: QuicConnectionConfig,
    mut commands: tokio::sync::mpsc::Receiver<Command>,
    started: mpsc::SyncSender<Result<WorkspaceId, BackendServiceError>>,
) {
    let mut core = client_core::ClientCore::default();
    let mut subscribers = Vec::new();
    let (mut socket, workspace) = match restore(&config, &mut core, &mut subscribers).await {
        Ok((client, workspace)) => (Some(client), workspace),
        Err(error) => {
            let _ = started.send(Err(error));
            return;
        }
    };
    // 一度解決した Workspace を再接続でも使い、接続中に選択先を変えない。
    config.workspace_id = Some(workspace.clone());
    if started.send(Ok(workspace)).is_err() {
        close(&mut socket).await;
        return;
    }
    loop {
        let command = if let Some(client) = socket.as_mut() {
            tokio::select! {
                command = commands.recv() => command,
                message = client.recv() => {
                    match message {
                        Ok(message) => match core.receive(message) {
                            Ok(Some(event)) => publish(&mut subscribers, event),
                            Ok(None) => {},
                            Err(_) => { socket = None; subscribers.clear(); }
                        },
                        _ => { core.disconnect(); socket = None; subscribers.clear(); }
                    }
                    continue;
                }
            }
        } else {
            commands.recv().await
        };
        let Some(command) = command else { break };
        match command {
            Command::Disconnect(reply) => {
                close(&mut socket).await;
                core.disconnect();
                subscribers.clear();
                let _ = reply.send(());
            }
            Command::Subscribe(reply) => {
                let (sender, receiver) = mpsc::sync_channel(256);
                subscribers.push(sender);
                let _ = reply.send(BackendEventReceiver::from_receiver(receiver));
            }
            Command::Request(body, deadline, reply) => {
                // まだ送信していない操作は、待ち手が破棄された後に実行しない。
                if reply.is_cancelled() {
                    continue;
                }
                if std::time::Instant::now() >= deadline {
                    reply.send(Err(BackendServiceError::Unavailable));
                    continue;
                }
                let result =
                    tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), async {
                        if socket.is_none() {
                            let (client, _) = restore(&config, &mut core, &mut subscribers).await?;
                            socket = Some(client);
                        }
                        if let Some(client) = socket.as_mut() {
                            exchange(client, body, &mut core, &mut subscribers).await
                        } else {
                            Err(BackendServiceError::Unavailable)
                        }
                    })
                    .await
                    .unwrap_or(Err(BackendServiceError::Unavailable));
                if result.is_err() {
                    close(&mut socket).await;
                    core.disconnect();
                    subscribers.clear();
                }
                reply.send(result);
            }
        }
    }
    close(&mut socket).await;
}

async fn close(socket: &mut Option<QuicClient>) {
    if let Some(client) = socket.take() {
        client.shutdown().await;
    }
}

async fn restore(
    config: &QuicConnectionConfig,
    core: &mut client_core::ClientCore,
    subscribers: &mut Vec<mpsc::SyncSender<EventEnvelope>>,
) -> Result<(QuicClient, WorkspaceId), BackendServiceError> {
    timeout(
        Duration::from_secs(30),
        restore_inner(config, core, subscribers),
    )
    .await
    .unwrap_or(Err(BackendServiceError::Unavailable))
}

async fn restore_inner(
    config: &QuicConnectionConfig,
    core: &mut client_core::ClientCore,
    subscribers: &mut Vec<mpsc::SyncSender<EventEnvelope>>,
) -> Result<(QuicClient, WorkspaceId), BackendServiceError> {
    let (mut client, hello) = config
        .authenticate()
        .await
        .map_err(|error| BackendServiceError::Connection(error.to_string()))?;
    core.connected()
        .map_err(|_| BackendServiceError::Unavailable)?;
    // Host の認証応答を優先し、指定も応答も無い場合だけ一覧の先頭を使う。
    let workspace = match hello
        .workspace_id
        .clone()
        .or_else(|| config.workspace_id.clone())
    {
        Some(workspace) => workspace,
        None => {
            let list = exchange(
                &mut client,
                RequestBody::WorkspaceList(WorkspaceListRequest::default()),
                core,
                subscribers,
            )
            .await?;
            match list {
                ResponseBody::WorkspaceList(list) => list
                    .workspaces
                    .first()
                    .map(|workspace| workspace.workspace_id.clone())
                    .ok_or_else(|| {
                        BackendServiceError::Connection(
                            "HostにWorkspaceが登録されていません".to_owned(),
                        )
                    })?,
                _ => {
                    return Err(BackendServiceError::Connection(
                        "Workspace一覧を取得できません".to_owned(),
                    ));
                }
            }
        }
    };
    let connected = exchange(
        &mut client,
        RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
            workspace_id: workspace.clone(),
        }),
        core,
        subscribers,
    )
    .await?;
    if !matches!(connected, ResponseBody::WorkspaceConnect(_)) {
        return Err(BackendServiceError::Connection(
            "Workspace接続が拒否されました".to_owned(),
        ));
    }
    // 再接続時は資源へのattachmentを復元する。更新要求の再送はしない。
    let snapshot = exchange(
        &mut client,
        RequestBody::SnapshotResync(SnapshotRequest {
            workspace_id: workspace.clone(),
            reason: SnapshotReason::Reconnect,
        }),
        core,
        subscribers,
    )
    .await?;
    if !matches!(snapshot, ResponseBody::SnapshotResync(_)) {
        return Err(BackendServiceError::Connection(
            "snapshot取得が拒否されました".to_owned(),
        ));
    }
    Ok((client, workspace))
}

fn publish(subscribers: &mut Vec<mpsc::SyncSender<EventEnvelope>>, event: EventEnvelope) {
    // 欠落時はreceiverを切断し、既存のsnapshot復旧へ移行させる。
    subscribers.retain(|sender| sender.try_send(event.clone()).is_ok());
}

async fn exchange(
    client: &mut QuicClient,
    body: RequestBody,
    core: &mut client_core::ClientCore,
    subscribers: &mut Vec<mpsc::SyncSender<EventEnvelope>>,
) -> Result<ResponseBody, BackendServiceError> {
    let (request, mut reply) = core
        .begin(body)
        .map_err(|_| BackendServiceError::Unavailable)?;
    let result = timeout(Duration::from_secs(35), async {
        client
            .send(&request)
            .await
            .map_err(|error| BackendServiceError::Connection(error.to_string()))?;
        loop {
            let message = client
                .recv()
                .await
                .map_err(|error| BackendServiceError::Connection(error.to_string()))?;
            if let Some(event) = core
                .receive(message)
                .map_err(|_| BackendServiceError::Unavailable)?
            {
                publish(subscribers, event);
            }
            match reply.try_recv() {
                Ok(Some(Ok(body))) => {
                    return Ok(body);
                }
                Ok(None) => {}
                _ => return Err(BackendServiceError::Unavailable),
            }
        }
    })
    .await;
    result.unwrap_or(Err(BackendServiceError::Unavailable))
}

#[cfg(test)]
mod tests {
    use super::RequestReply;
    use super::*;
    use crate::{
        AuthConfig, AuthPolicy, CredentialLifetime, PairingLifetime, QuicServer, RemoteAuth,
        RemoteRequestHandler, RemoteResponseFuture, SessionGrant, tls13_server_config,
    };
    use rustls::pki_types::PrivateKeyDer;
    use std::sync::Mutex;

    struct NoRequests;

    impl RemoteRequestHandler for NoRequests {
        fn handle(
            &self,
            _: SessionGrant,
            _: client_api::RequestEnvelope,
        ) -> RemoteResponseFuture<'_> {
            Box::pin(async { panic!("authentication must not dispatch requests") })
        }
    }

    fn hello() -> HelloRequest {
        HelloRequest {
            protocol: ProtocolRange::exact(CURRENT_PROTOCOL),
            client_id: ClientId::try_new("desktop-oidc-test").unwrap(),
            client_name: "Desktop".to_owned(),
            client_kind: ClientKind::Desktop,
            requested_capabilities: Default::default(),
        }
    }

    #[test]
    fn cancelled_async_request_is_detected_before_dispatch() {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let reply = RequestReply::Async(sender);
        assert!(!reply.is_cancelled());
        drop(receiver);
        assert!(reply.is_cancelled());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn oidc_connection_authenticates_with_a_stored_id_token() {
        let auth = Arc::new(Mutex::new(RemoteAuth::system(
            AuthConfig::new(
                PairingLifetime::new(10).unwrap(),
                CredentialLifetime::new(20).unwrap(),
            )
            .with_auto_approve_oidc_devices(true),
            AuthPolicy::new(Default::default()),
        )));
        {
            let mut auth = auth.lock().unwrap();
            auth.enable();
            auth.enable_oidc(
                Arc::new(crate::oidc::test_verifier()),
                client_api::OidcClientConfig {
                    issuer: "https://idp.example".to_owned(),
                    client_id: "lapis".to_owned(),
                    scopes: vec!["openid".to_owned()],
                },
            );
        }
        let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        let certificate = certified.cert.der().clone();
        let tls = tls13_server_config(
            vec![certificate.clone()],
            PrivateKeyDer::Pkcs8(certified.signing_key.serialize_der().into()),
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
        let id_token =
            crate::oidc::test_token("alice", jsonwebtoken::get_current_timestamp() + 300);
        let config = QuicConnectionConfig::oidc(
            address,
            "localhost".to_owned(),
            certificate,
            hello(),
            Some(WorkspaceId::try_new("workspace-1").unwrap()),
            id_token.clone(),
        );
        assert_eq!(config.id_token(), Some(id_token.as_str()));
        let (client, hello) = config.authenticate().await.unwrap();
        assert_eq!(hello.subject.as_deref(), Some("alice"));
        drop(client);
        server.shutdown().await;
    }
}
