use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};

use crate::ClientCore;
use client_api::{
    AuthenticateRequest, CREDENTIAL_EXPIRED, CURRENT_PROTOCOL, CapabilityId, CapabilitySet,
    ClientId, ClientKind, ClientMessage, CredentialId, EventEnvelope, HOST_MISMATCH, HelloRequest,
    OidcRequest, PairRequest, ProtocolRange, RequestBody, ResponseBody, ServerMessage,
    WORKSPACE_MISMATCH, WORKSPACE_UNAVAILABLE, WorkspaceId,
};
use futures_channel::{mpsc, oneshot};
use futures_util::future::{Either, select};
use gloo_timers::future::TimeoutFuture;
use std::pin::pin;
use web_transport_wasm::{RecvStream, SendStream, Session};
use zeroize::Zeroize;

type Reply<T> = oneshot::Sender<Result<T, String>>;

/// フレームの長さ（4 byte big-endian）+ UTF-8 JSON。Host の QUIC 経路と同じ。
type Frame = Vec<u8>;

fn encode_frame(bytes: &[u8]) -> Frame {
    let mut frame = Vec::with_capacity(4 + bytes.len());
    frame.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    frame.extend_from_slice(bytes);
    frame
}

/// 受信バッファから完成したフレームを取り出す。不足なら `None`。
fn take_frame(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    if buffer.len() < 4 {
        return None;
    }
    let length = u32::from_be_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]) as usize;
    if buffer.len() < 4 + length {
        return None;
    }
    let frame = buffer[4..4 + length].to_vec();
    buffer.drain(..4 + length);
    Some(frame)
}

struct Shared {
    core: ClientCore,
    opened: Option<Reply<()>>,
    auth_required: Option<Reply<()>>,
    authentication: Option<Reply<ServerMessage>>,
    events: mpsc::Sender<EventEnvelope>,
    authenticated: bool,
    pairing: bool,
    /// Host が認証応答で割り当てた Session。
    session_id: Option<client_api::SessionId>,
}

impl Shared {
    fn fail(&mut self, message: &str) {
        self.core.disconnect();
        self.authenticated = false;
        if let Some(reply) = self.opened.take() {
            let _ = reply.send(Err(message.into()));
        }
        if let Some(reply) = self.auth_required.take() {
            let _ = reply.send(Err(message.into()));
        }
        if let Some(reply) = self.authentication.take() {
            let _ = reply.send(Err(message.into()));
        }
        self.events.close_channel();
    }
}

/// WebTransport セッションと共通 Client の寿命を一緒に管理する。
///
/// Desktop と同じ長さ付きフレームで `ClientMessage` / `ServerMessage` を運ぶ。
pub struct BrowserConnection {
    session: Session,
    writer: Rc<RefCell<SendStream>>,
    shared: Rc<RefCell<Shared>>,
    events: Option<mpsc::Receiver<EventEnvelope>>,
}

struct RequestGuard {
    shared: Rc<RefCell<Shared>>,
    armed: bool,
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        if self.armed {
            self.shared.borrow_mut().fail("要求が中断されました");
        }
    }
}

impl BrowserConnection {
    /// 保持中の購読を解除し、接続を明示的に終了する。
    pub fn disconnect(&self) {
        self.shared.borrow_mut().fail("WebTransportが終了しました");
        self.session.close(0, "client disconnected");
    }

    /// 認証応答で Host が割り当てた Session。認証前は `None`。
    pub fn session_id(&self) -> Option<client_api::SessionId> {
        self.shared.borrow().session_id.clone()
    }

    /// 保存済み credential または pairing token で認証する。
    ///
    /// `endpoint` は `https://<host>:<port>/lapis` と、Host の証明書 DER の SHA-256。
    /// ブラウザーはこのハッシュと一致する自己署名証明書だけを受け付ける。
    pub async fn connect(
        endpoint: &crate::browser_endpoint::WebTransportEndpoint,
        auth: ClientMessage,
    ) -> Result<(Self, ServerMessage), String> {
        Self::connect_inner(endpoint, None, auth).await
    }

    /// Hello で認証方式を問い合わせてから認証する。
    pub async fn connect_with_hello(
        endpoint: &crate::browser_endpoint::WebTransportEndpoint,
        hello: HelloRequest,
        auth: ClientMessage,
    ) -> Result<(Self, ServerMessage), String> {
        Self::connect_inner(endpoint, Some(hello), auth).await
    }

    async fn connect_inner(
        endpoint: &crate::browser_endpoint::WebTransportEndpoint,
        hello: Option<HelloRequest>,
        auth: ClientMessage,
    ) -> Result<(Self, ServerMessage), String> {
        let pairing = match &auth {
            ClientMessage::Pair(_) => true,
            ClientMessage::Authenticate(_) | ClientMessage::Oidc(_) => false,
            _ => return Err("認証要求が必要です".into()),
        };
        let url = endpoint
            .url
            .parse::<url::Url>()
            .map_err(|_| "WebTransportのURLが不正です")?;
        // 証明書は配布された DER のハッシュだけを信頼する。ルート CA は使わない。
        let client = web_transport_wasm::ClientBuilder::new()
            .with_pooling(false)
            .with_server_certificate_hashes(vec![endpoint.certificate_sha256.clone()]);
        // `Client::connect` の future は Unpin ではないため固定してから待つ。
        let connect = pin!(client.connect(url));
        let session = match select(connect, TimeoutFuture::new(20_000)).await {
            Either::Left((Ok(session), _)) => session,
            Either::Left((Err(_), _)) => return Err("WebTransportに接続できません".into()),
            Either::Right(_) => return Err("WebTransportの接続がtimeoutしました".into()),
        };
        let (send, recv) = session
            .open_bi()
            .await
            .map_err(|_| "WebTransportのstreamを開始できません")?;

        let (opened, wait_open) = oneshot::channel();
        let (auth_required, wait_auth_required) = oneshot::channel();
        let (authentication, wait_auth) = oneshot::channel();
        let (events_tx, events) = mpsc::channel(256);
        let shared = Rc::new(RefCell::new(Shared {
            core: ClientCore::default(),
            opened: Some(opened),
            auth_required: hello.is_some().then_some(auth_required),
            authentication: Some(authentication),
            events: events_tx,
            authenticated: false,
            pairing,
            session_id: None,
        }));
        if let Some(reply) = shared.borrow_mut().opened.take() {
            let _ = reply.send(Ok(()));
        }

        // 受信は接続の寿命と同じ task で回し、切断を購読の失敗として通知する。
        {
            let shared = Rc::downgrade(&shared);
            wasm_bindgen_futures::spawn_local(async move {
                receive_loop(recv, shared).await;
            });
        }

        let connection = Self {
            session,
            writer: Rc::new(RefCell::new(send)),
            shared,
            events: Some(events),
        };
        wait(wait_open, 20_000, "WebTransportの開始がtimeoutしました").await?;
        if let Some(hello) = hello {
            connection
                .send_message(ClientMessage::Hello(hello))
                .await
                .map_err(|error| error.to_string())?;
            wait(
                wait_auth_required,
                10_000,
                "認証方式の取得がtimeoutしました",
            )
            .await?;
        }
        // 認証要求は secret を含むため、送信後にバッファを消す。
        let mut encoded = serde_json::to_vec(&auth).map_err(|_| "認証要求を生成できません")?;
        let result = connection.send_bytes(&encoded).await;
        use zeroize::Zeroize as _;
        encoded.zeroize();
        result.map_err(|_| "認証要求を送信できません")?;
        let response = wait(wait_auth, 30_000, "認証がtimeoutしました").await?;
        Ok((connection, response))
    }

    async fn send_message(&self, message: ClientMessage) -> Result<(), String> {
        let bytes = serde_json::to_vec(&message).map_err(|_| "要求を生成できません")?;
        self.send_bytes(&bytes).await
    }

    async fn send_bytes(&self, bytes: &[u8]) -> Result<(), String> {
        let frame = encode_frame(bytes);
        let mut writer = self.writer.borrow_mut();
        writer
            .write(&frame)
            .await
            .map_err(|_| "要求を送信できません".to_owned())
    }

    pub async fn request(&self, body: RequestBody) -> Result<ResponseBody, String> {
        let (message, reply) = self
            .shared
            .borrow_mut()
            .core
            .begin(body)
            .map_err(|error| error.to_string())?;
        if let Err(error) = self.send_message(message).await {
            self.shared.borrow_mut().fail(&error);
            self.session.close(0, "send failed");
            return Err(error);
        }
        // 送信済み要求のfutureが破棄されたら結果不明の接続を再利用しない。
        let mut guard = RequestGuard {
            shared: self.shared.clone(),
            armed: true,
        };
        let body = match select(reply, TimeoutFuture::new(35_000)).await {
            Either::Left((Ok(Ok(body)), _)) => body,
            _ => {
                self.shared
                    .borrow_mut()
                    .fail("要求が失敗またはtimeoutしました");
                self.session.close(0, "request failed");
                return Err("要求が失敗またはtimeoutしました".into());
            }
        };
        guard.armed = false;
        Ok(body)
    }

    /// 購読受信と要求を独立したtaskで同時に処理する。
    pub fn take_events(&mut self) -> Option<mpsc::Receiver<EventEnvelope>> {
        self.events.take()
    }
}

/// 受信ループ。フレーム単位で `ServerMessage` を取り出し、共通 Client へ渡す。
async fn receive_loop(mut recv: RecvStream, shared: Weak<RefCell<Shared>>) {
    let mut buffer = Vec::new();
    loop {
        let chunk = match recv.read(64 * 1024).await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(_) => break,
        };
        buffer.extend_from_slice(&chunk);
        while let Some(frame) = take_frame(&mut buffer) {
            let Some(shared) = shared.upgrade() else {
                return;
            };
            let message = match serde_json::from_slice::<ServerMessage>(&frame) {
                Ok(message) => message,
                Err(_) => {
                    shared.borrow_mut().fail("Host応答の形式が不正です");
                    return;
                }
            };
            if let Err(error) = handle_message(&mut shared.borrow_mut(), message) {
                shared.borrow_mut().fail(&error);
                return;
            }
        }
    }
    if let Some(shared) = shared.upgrade() {
        shared.borrow_mut().fail("WebTransportが切断されました");
    }
}

async fn wait<T>(
    reply: oneshot::Receiver<Result<T, String>>,
    timeout_ms: u32,
    error: &str,
) -> Result<T, String> {
    match select(reply, TimeoutFuture::new(timeout_ms)).await {
        Either::Left((Ok(result), _)) => result,
        _ => Err(error.to_owned()),
    }
}

/// ページが保存する credential。secret そのものを含む。
/// Host が永続化する digest だけの `StoredCredential` とは別の概念。
#[derive(serde::Deserialize, serde::Serialize)]
pub struct SavedCredential {
    pub id: String,
    pub secret: String,
    /// 発行元 Host。次回接続時に別 Host を早期に拒否する。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_id: Option<String>,
}

/// 認証まで完了した Web 接続。Host/Workspace の識別子を UI へ渡す。
pub struct WebConnection {
    pub connection: BrowserConnection,
    pub credential: Option<String>,
    pub workspace_id: Option<WorkspaceId>,
    pub host_id: Option<String>,
    pub capabilities: CapabilitySet,
}

/// WebTransport 経由の Client 接続設定。
pub struct WebClientConnectOptions<'a> {
    /// WebTransport の接続先と、信頼する証明書ハッシュ。
    pub endpoint: &'a crate::browser_endpoint::WebTransportEndpoint,
    pub workspace: Option<WorkspaceId>,
    pub client_id: &'a str,
    pub client_name: &'a str,
    pub client_kind: ClientKind,
    pub token: Option<&'a str>,
    pub credential: Option<&'a str>,
    pub id_token: Option<&'a str>,
    pub capabilities: &'a [&'a str],
}

/// WebTransport 経由で Host へ接続し、OIDC / credential / pairing のいずれかで認証を完了する。
///
/// OIDC Token が指定された場合は他の方式へ暗黙にフォールバックしない。
/// `credential` が失敗し `token` もある場合はpairingへフォールバックする。
/// 返り値の `Some` はpairing直後のcredential JSON。
pub async fn connect_web_client(
    options: WebClientConnectOptions<'_>,
) -> Result<WebConnection, String> {
    let WebClientConnectOptions {
        endpoint,
        workspace,
        client_id,
        client_name,
        client_kind,
        token,
        credential,
        id_token,
        capabilities,
    } = options;
    let hello = client_hello(client_id, client_name, client_kind, capabilities)?;
    let can_retry_pair = id_token.is_none() && credential.is_some() && token.is_some();
    let auth = if let Some(id_token) = id_token {
        match workspace.clone() {
            Some(workspace) => ClientMessage::Oidc(OidcRequest::new(
                hello.clone(),
                workspace,
                id_token.to_owned(),
            )),
            None => ClientMessage::Oidc(OidcRequest::new_auto(hello.clone(), id_token.to_owned())),
        }
    } else {
        match credential {
            Some(credential) => {
                let stored: SavedCredential = serde_json::from_str(credential)
                    .map_err(|_| "credentialの形式が不正です".to_owned())?;
                let id = CredentialId::parse(stored.id)
                    .map_err(|_| "credential IDが不正です".to_owned())?;
                let mut secret = decode_hex(&stored.secret)?;
                let request = match workspace.clone() {
                    Some(workspace) => {
                        AuthenticateRequest::new(hello.clone(), workspace, &id, &secret)
                    }
                    None => AuthenticateRequest::new_auto(hello.clone(), &id, &secret),
                }
                .with_host_id(stored.host_id);
                secret.zeroize();
                ClientMessage::Authenticate(request)
            }
            None => {
                let token = token.ok_or_else(|| "pairing tokenが必要です".to_owned())?;
                let mut secret = decode_hex(token)?;
                let request = PairRequest::new(hello.clone(), &secret);
                secret.zeroize();
                ClientMessage::Pair(request)
            }
        }
    };
    let (connection, response) =
        match BrowserConnection::connect_with_hello(endpoint, hello.clone(), auth).await {
            Ok(result) => result,
            Err(_) if can_retry_pair => {
                let token = token.ok_or_else(|| "pairing tokenが必要です".to_owned())?;
                let mut secret = decode_hex(token)?;
                let retry_hello = hello.clone();
                let request = PairRequest::new(hello, &secret);
                secret.zeroize();
                BrowserConnection::connect_with_hello(
                    endpoint,
                    retry_hello,
                    ClientMessage::Pair(request),
                )
                .await?
            }
            Err(error) => return Err(error),
        };
    let (credential, selected_workspace, host_id, capabilities) =
        credential_from_response(response)?;
    Ok(WebConnection {
        connection,
        credential,
        workspace_id: selected_workspace,
        host_id,
        capabilities,
    })
}

fn client_hello(
    client_id: &str,
    client_name: &str,
    client_kind: ClientKind,
    capabilities: &[&str],
) -> Result<HelloRequest, String> {
    let capabilities = capabilities
        .iter()
        .map(|capability| {
            CapabilityId::try_new(*capability).map_err(|_| "capabilityが不正です".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(HelloRequest {
        protocol: ProtocolRange::exact(CURRENT_PROTOCOL),
        client_id: ClientId::try_new(client_id).map_err(|_| "Client IDが不正です".to_owned())?,
        client_name: client_name.to_owned(),
        client_kind,
        requested_capabilities: CapabilitySet::try_new(capabilities)
            .map_err(|_| "capabilityが不正です".to_owned())?,
    })
}

fn credential_from_response(
    response: ServerMessage,
) -> Result<
    (
        Option<String>,
        Option<WorkspaceId>,
        Option<String>,
        CapabilitySet,
    ),
    String,
> {
    match response {
        ServerMessage::Paired(mut paired) => {
            let workspace_id = paired.hello.workspace_id.clone();
            let host_id = paired.hello.host_id.clone();
            let capabilities = paired.hello.granted_capabilities.clone();
            let id = paired
                .credential_id()
                .map_err(|_| "credential IDが不正です".to_owned())?;
            let secret = paired
                .take_secret()
                .map_err(|_| "credential secretが不正です".to_owned())?;
            let stored = SavedCredential {
                id: id.as_str().to_owned(),
                secret: encode_hex(&secret),
                host_id: host_id.clone(),
            };
            Ok((
                Some(
                    serde_json::to_string(&stored)
                        .map_err(|_| "credentialを保存できません".to_owned())?,
                ),
                workspace_id,
                host_id,
                capabilities,
            ))
        }
        ServerMessage::Authenticated(hello) => Ok((
            None,
            hello.workspace_id,
            hello.host_id,
            hello.granted_capabilities,
        )),
        _ => Err("認証応答が不正です".to_owned()),
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn decode_hex(value: &str) -> Result<[u8; 32], String> {
    let value = value.trim();
    if value.len() != 64 {
        return Err("secretの長さが不正です".to_owned());
    }
    let mut output = [0u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let digit = |byte: u8| match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            _ => None,
        };
        let high = digit(pair[0]).ok_or_else(|| "secretが不正です".to_owned())?;
        let low = digit(pair[1]).ok_or_else(|| "secretが不正です".to_owned())?;
        output[index] = (high << 4) | low;
    }
    Ok(output)
}

fn authentication_error(code: &str) -> String {
    match code {
        CREDENTIAL_EXPIRED => {
            "credentialの有効期限が切れています。pairing tokenで再接続してください".to_owned()
        }
        WORKSPACE_MISMATCH => {
            "指定したWorkspaceは、このcredentialの接続先ではありません".to_owned()
        }
        WORKSPACE_UNAVAILABLE => "指定したWorkspaceはHostに登録されていません".to_owned(),
        HOST_MISMATCH => "保存しているcredentialは別のHostのものです".to_owned(),
        other => format!("認証が拒否されました: {other}"),
    }
}

fn handle_message(shared: &mut Shared, message: ServerMessage) -> Result<(), String> {
    if !shared.authenticated {
        if let ServerMessage::AuthRequired(required) = &message {
            if required.methods.is_empty() {
                return Err("認証方式が提示されませんでした".into());
            }
            let reply = shared
                .auth_required
                .take()
                .ok_or("認証方式の提示が重複しました")?;
            let _ = reply.send(Ok(()));
            return Ok(());
        }
        let hello = match &message {
            ServerMessage::Paired(response) if shared.pairing => &response.hello,
            ServerMessage::Authenticated(hello) if !shared.pairing => hello,
            ServerMessage::Error(error) => {
                return Err(authentication_error(error.code.as_str()));
            }
            _ => return Err("認証応答が不正です".into()),
        };
        if hello.protocol != CURRENT_PROTOCOL {
            return Err("protocolが一致しません".into());
        }
        shared
            .core
            .connected()
            .map_err(|_| "Clientを開始できません")?;
        shared.authenticated = true;
        shared.session_id = Some(hello.session_id.clone());
        let reply = shared
            .authentication
            .take()
            .ok_or("認証応答が重複しました")?;
        let _ = reply.send(Ok(message));
        return Ok(());
    }
    if let Some(event) = shared
        .core
        .receive(message)
        .map_err(|_| "Hostとの同期が途切れました")?
    {
        shared
            .events
            .try_send(event)
            .map_err(|_| "イベントが蓄積上限を超えました")?;
    }
    Ok(())
}

impl Drop for BrowserConnection {
    fn drop(&mut self) {
        self.shared.borrow_mut().fail("WebTransportが終了しました");
        self.session.close(0, "client disconnected");
    }
}

impl crate::AsyncHostConnection for BrowserConnection {
    async fn request(&self, body: RequestBody) -> Result<ResponseBody, String> {
        BrowserConnection::request(self, body).await
    }
}
