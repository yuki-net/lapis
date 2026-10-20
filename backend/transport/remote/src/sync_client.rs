use crate::{
    AuthenticateRequest, ClientMessage, CredentialHandle, PairRequest, QuicClient, RemoteLimits,
    ServerMessage,
};
use lapis_backend_state::{BackendEventReceiver, BackendServiceError};
use lapis_client_api::*;
use rustls::pki_types::CertificateDer;
use std::{
    net::SocketAddr,
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};
use tokio::time::timeout;

type Error = Box<dyn std::error::Error + Send + Sync>;

#[derive(Clone)]
pub struct QuicConnectionConfig {
    pub address: SocketAddr,
    pub server_name: String,
    pub certificate: CertificateDer<'static>,
    pub hello: ClientHello,
    pub workspace_id: WorkspaceId,
    pub credential: Arc<CredentialHandle>,
}

impl QuicConnectionConfig {
    /// 招待はこの一回だけ交換し、再接続には発行済みcredentialを使う。
    pub async fn pair(
        address: SocketAddr,
        server_name: String,
        certificate: CertificateDer<'static>,
        hello: ClientHello,
        workspace_id: WorkspaceId,
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
            credential,
        })
    }

    async fn authenticate(&self) -> Result<QuicClient, Error> {
        let mut client = QuicClient::connect(
            self.address,
            &self.server_name,
            self.certificate.clone(),
            RemoteLimits::default(),
        )
        .await?;
        client
            .send(&ClientMessage::Authenticate(AuthenticateRequest::new(
                self.hello.clone(),
                self.workspace_id.clone(),
                self.credential.id(),
                self.credential.secret_bytes(),
            )))
            .await?;
        let message = timeout(Duration::from_secs(10), client.recv()).await??;
        match message {
            ServerMessage::Authenticated(hello) if hello.protocol == CURRENT_PROTOCOL => Ok(client),
            _ => Err("Hostの認証が拒否されました".into()),
        }
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
    Request(
        RequestBody,
        std::time::Instant,
        mpsc::SyncSender<Result<ResponseBody, BackendServiceError>>,
    ),
    Subscribe(mpsc::SyncSender<BackendEventReceiver>),
    Disconnect(mpsc::SyncSender<()>),
}

impl QuicConnection {
    pub fn connect(config: QuicConnectionConfig) -> Result<Self, BackendServiceError> {
        let workspace = config.workspace_id.clone();
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
                        let _ = started_tx.send(false);
                        return;
                    }
                };
                runtime.block_on(run(config, receiver, started_tx));
            })
            .map_err(BackendServiceError::ThreadSpawn)?;
        match started_rx.recv_timeout(Duration::from_secs(45)) {
            Ok(true) => Ok(Self {
                inner: Arc::new(ConnectionInner { workspace, sender }),
            }),
            _ => Err(BackendServiceError::Unavailable),
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
                sender,
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

async fn run(
    config: QuicConnectionConfig,
    mut commands: tokio::sync::mpsc::Receiver<Command>,
    started: mpsc::SyncSender<bool>,
) {
    let mut sequence = 0u64;
    let mut subscribers = Vec::new();
    let mut socket = restore(&config, &mut sequence, &mut subscribers).await.ok();
    if started.send(socket.is_some()).is_err() || socket.is_none() {
        return;
    }
    loop {
        let command = if let Some(client) = socket.as_mut() {
            tokio::select! {
                command = commands.recv() => command,
                message = client.recv() => {
                    match message {
                        Ok(ServerMessage::Event(event)) => publish(&mut subscribers, event),
                        _ => { socket = None; subscribers.clear(); }
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
                socket = None;
                subscribers.clear();
                let _ = reply.send(());
            }
            Command::Subscribe(reply) => {
                let (sender, receiver) = mpsc::sync_channel(256);
                subscribers.push(sender);
                let _ = reply.send(BackendEventReceiver::from_receiver(receiver));
            }
            Command::Request(body, deadline, reply) => {
                if std::time::Instant::now() >= deadline {
                    let _ = reply.send(Err(BackendServiceError::Unavailable));
                    continue;
                }
                let result =
                    tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), async {
                        if socket.is_none() {
                            socket = restore(&config, &mut sequence, &mut subscribers).await.ok();
                        }
                        if let Some(client) = socket.as_mut() {
                            exchange(client, body, &mut sequence, &mut subscribers).await
                        } else {
                            Err(BackendServiceError::Unavailable)
                        }
                    })
                    .await
                    .unwrap_or(Err(BackendServiceError::Unavailable));
                if result.is_err() {
                    socket = None;
                    subscribers.clear();
                }
                let _ = reply.send(result);
            }
        }
    }
}

async fn restore(
    config: &QuicConnectionConfig,
    sequence: &mut u64,
    subscribers: &mut Vec<mpsc::SyncSender<EventEnvelope>>,
) -> Result<QuicClient, BackendServiceError> {
    timeout(
        Duration::from_secs(30),
        restore_inner(config, sequence, subscribers),
    )
    .await
    .unwrap_or(Err(BackendServiceError::Unavailable))
}

async fn restore_inner(
    config: &QuicConnectionConfig,
    sequence: &mut u64,
    subscribers: &mut Vec<mpsc::SyncSender<EventEnvelope>>,
) -> Result<QuicClient, BackendServiceError> {
    let mut client = config
        .authenticate()
        .await
        .map_err(|_| BackendServiceError::Unavailable)?;
    let connected = exchange(
        &mut client,
        RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
            workspace_id: config.workspace_id.clone(),
        }),
        sequence,
        subscribers,
    )
    .await?;
    if !matches!(connected, ResponseBody::WorkspaceConnect(_)) {
        return Err(BackendServiceError::Unavailable);
    }
    // 再接続時は資源へのattachmentを復元する。更新要求の再送はしない。
    let snapshot = exchange(
        &mut client,
        RequestBody::SnapshotResync(SnapshotRequest {
            workspace_id: config.workspace_id.clone(),
            reason: SnapshotReason::Reconnect,
        }),
        sequence,
        subscribers,
    )
    .await?;
    if !matches!(snapshot, ResponseBody::SnapshotResync(_)) {
        return Err(BackendServiceError::Unavailable);
    }
    Ok(client)
}

fn publish(subscribers: &mut Vec<mpsc::SyncSender<EventEnvelope>>, event: EventEnvelope) {
    // 欠落時はreceiverを切断し、既存のsnapshot復旧へ移行させる。
    subscribers.retain(|sender| sender.try_send(event.clone()).is_ok());
}

async fn exchange(
    client: &mut QuicClient,
    body: RequestBody,
    sequence: &mut u64,
    subscribers: &mut Vec<mpsc::SyncSender<EventEnvelope>>,
) -> Result<ResponseBody, BackendServiceError> {
    *sequence = sequence
        .checked_add(1)
        .ok_or(BackendServiceError::Unavailable)?;
    let request = RequestEnvelope {
        request_id: RequestId::try_new(format!("quic-{sequence}")).expect("bounded request id"),
        body,
    };
    let result = timeout(Duration::from_secs(35), async {
        client
            .send(&ClientMessage::Request(request.clone()))
            .await
            .map_err(|_| BackendServiceError::Unavailable)?;
        loop {
            match client
                .recv()
                .await
                .map_err(|_| BackendServiceError::Unavailable)?
            {
                ServerMessage::Event(event) => publish(subscribers, event),
                ServerMessage::Response(response) if response.matches_request(&request) => {
                    return Ok(response.body);
                }
                _ => return Err(BackendServiceError::Unavailable),
            }
        }
    })
    .await;
    result.unwrap_or(Err(BackendServiceError::Unavailable))
}
