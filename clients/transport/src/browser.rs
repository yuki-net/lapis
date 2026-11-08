use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};

use crate::ClientCore;
use client_api::{
    AuthenticateRequest, CURRENT_PROTOCOL, CapabilityId, CapabilitySet, ClientId, ClientKind,
    ClientMessage, CredentialId, EventEnvelope, HelloRequest, OidcRequest, PairRequest,
    ProtocolRange, RequestBody, ResponseBody, ServerMessage, WorkspaceId,
};
use futures_channel::{mpsc, oneshot};
use futures_util::future::{Either, select};
use gloo_timers::future::TimeoutFuture;
use wasm_bindgen::{JsCast, closure::Closure};
use web_sys::{Event, MessageEvent, WebSocket};
use zeroize::Zeroize;

type Reply<T> = oneshot::Sender<Result<T, String>>;

struct Shared {
    core: ClientCore,
    opened: Option<Reply<()>>,
    auth_required: Option<Reply<()>>,
    authentication: Option<Reply<ServerMessage>>,
    events: mpsc::Sender<EventEnvelope>,
    authenticated: bool,
    pairing: bool,
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

/// Browser WebSocketと共通Clientの寿命を一緒に管理する。
pub struct BrowserConnection {
    socket: WebSocket,
    shared: Rc<RefCell<Shared>>,
    events: Option<mpsc::Receiver<EventEnvelope>>,
    _onopen: Closure<dyn FnMut(Event)>,
    _onmessage: Closure<dyn FnMut(MessageEvent)>,
    _onclose: Closure<dyn FnMut(Event)>,
    _onerror: Closure<dyn FnMut(Event)>,
}

struct RequestGuard {
    socket: WebSocket,
    shared: Rc<RefCell<Shared>>,
    armed: bool,
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        if self.armed {
            self.shared.borrow_mut().fail("要求が中断されました");
            let _ = self.socket.close();
        }
    }
}

impl BrowserConnection {
    /// ペアリングまたは保存済みcredentialで認証する。要求は認証後だけ受け付ける。
    pub async fn connect(url: &str, auth: ClientMessage) -> Result<(Self, ServerMessage), String> {
        Self::connect_inner(url, None, auth).await
    }

    /// Helloで認証方式を問い合わせてから認証する。
    pub async fn connect_with_hello(
        url: &str,
        hello: HelloRequest,
        auth: ClientMessage,
    ) -> Result<(Self, ServerMessage), String> {
        Self::connect_inner(url, Some(hello), auth).await
    }

    async fn connect_inner(
        url: &str,
        hello: Option<HelloRequest>,
        auth: ClientMessage,
    ) -> Result<(Self, ServerMessage), String> {
        let pairing = match &auth {
            ClientMessage::Pair(_) => true,
            ClientMessage::Authenticate(_) | ClientMessage::Oidc(_) => false,
            _ => return Err("認証要求が必要です".into()),
        };
        let socket = WebSocket::new_with_str(url, wire::REMOTE_WEBSOCKET_PROTOCOL)
            .map_err(|_| "WebSocketを開始できません")?;
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
        }));

        let weak = Rc::downgrade(&shared);
        let onopen = Closure::wrap(Box::new(move |_: Event| {
            if let Some(shared) = weak.upgrade()
                && let Some(reply) = shared.borrow_mut().opened.take()
            {
                let _ = reply.send(Ok(()));
            }
        }) as Box<dyn FnMut(Event)>);
        socket.set_onopen(Some(onopen.as_ref().unchecked_ref()));

        let weak = Rc::downgrade(&shared);
        let message_socket = socket.clone();
        let onmessage = Closure::wrap(Box::new(move |event: MessageEvent| {
            let Some(shared) = weak.upgrade() else { return };
            let result = event
                .data()
                .as_string()
                .ok_or_else(|| "WebSocketの受信形式が不正です".to_owned())
                .and_then(|text| {
                    serde_json::from_str::<ServerMessage>(&text)
                        .map_err(|_| "Host応答の形式が不正です".to_owned())
                })
                .and_then(|message| handle_message(&mut shared.borrow_mut(), message));
            if let Err(error) = result {
                shared.borrow_mut().fail(&error);
                let _ = message_socket.close();
            }
        }) as Box<dyn FnMut(MessageEvent)>);
        socket.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));

        let onclose = failure_handler(Rc::downgrade(&shared), "WebSocketが切断されました");
        socket.set_onclose(Some(onclose.as_ref().unchecked_ref()));
        let onerror = failure_handler(Rc::downgrade(&shared), "WebSocketの通信に失敗しました");
        socket.set_onerror(Some(onerror.as_ref().unchecked_ref()));
        let connection = Self {
            socket,
            shared,
            events: Some(events),
            _onopen: onopen,
            _onmessage: onmessage,
            _onclose: onclose,
            _onerror: onerror,
        };
        wait(wait_open, 10_000, "WebSocketの開始がtimeoutしました").await?;
        if let Some(hello) = hello {
            let encoded = serde_json::to_string(&ClientMessage::Hello(hello))
                .map_err(|_| "Helloを生成できません")?;
            connection
                .socket
                .send_with_str(&encoded)
                .map_err(|_| "Helloを送信できません")?;
            wait(
                wait_auth_required,
                10_000,
                "認証方式の取得がtimeoutしました",
            )
            .await?;
        }
        let mut encoded = serde_json::to_string(&auth).map_err(|_| "認証要求を生成できません")?;
        let result = connection
            .socket
            .send_with_str(&encoded)
            .map_err(|_| "認証要求を送信できません");
        use zeroize::Zeroize;
        encoded.zeroize();
        result?;
        let response = wait(wait_auth, 30_000, "認証がtimeoutしました").await?;
        Ok((connection, response))
    }

    pub async fn request(&self, body: RequestBody) -> Result<ResponseBody, String> {
        let (message, reply) = self
            .shared
            .borrow_mut()
            .core
            .begin(body)
            .map_err(|error| error.to_string())?;
        let encoded = match serde_json::to_string(&message) {
            Ok(encoded) => encoded,
            Err(_) => {
                self.shared.borrow_mut().fail("要求を生成できません");
                let _ = self.socket.close();
                return Err("要求を生成できません".into());
            }
        };
        if self.socket.send_with_str(&encoded).is_err() {
            self.shared.borrow_mut().fail("要求を送信できません");
            let _ = self.socket.close();
            return Err("要求を送信できません".into());
        }
        // 送信済み要求のfutureが破棄されたら結果不明の接続を再利用しない。
        let mut guard = RequestGuard {
            socket: self.socket.clone(),
            shared: self.shared.clone(),
            armed: true,
        };
        let body = match select(reply, TimeoutFuture::new(35_000)).await {
            Either::Left((Ok(Ok(body)), _)) => body,
            _ => {
                self.shared
                    .borrow_mut()
                    .fail("要求が失敗またはtimeoutしました");
                let _ = self.socket.close();
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

fn failure_handler(
    shared: Weak<RefCell<Shared>>,
    message: &'static str,
) -> Closure<dyn FnMut(Event)> {
    Closure::wrap(Box::new(move |_: Event| {
        if let Some(shared) = shared.upgrade() {
            shared.borrow_mut().fail(message);
        }
    }) as Box<dyn FnMut(Event)>)
}

/// ページが保存するcredential。
#[derive(serde::Deserialize, serde::Serialize)]
pub struct StoredCredential {
    pub id: String,
    pub secret: String,
}

/// WebSocket経由のClient接続設定。
pub struct WebClientConnectOptions<'a> {
    pub url: &'a str,
    pub workspace: Option<WorkspaceId>,
    pub client_id: &'a str,
    pub client_name: &'a str,
    pub client_kind: ClientKind,
    pub token: Option<&'a str>,
    pub credential: Option<&'a str>,
    pub id_token: Option<&'a str>,
    pub capabilities: &'a [&'a str],
}

/// WebSocket経由でHostへ接続し、OIDC / credential / pairing のいずれかで認証を完了する。
///
/// OIDC Token が指定された場合は他の方式へ暗黙にフォールバックしない。
/// `credential` が失敗し `token` もある場合はpairingへフォールバックする。
/// 返り値の `Some` はpairing直後のcredential JSON。
pub async fn connect_web_client(
    options: WebClientConnectOptions<'_>,
) -> Result<(BrowserConnection, Option<String>, Option<WorkspaceId>), String> {
    let WebClientConnectOptions {
        url,
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
                let stored: StoredCredential = serde_json::from_str(credential)
                    .map_err(|_| "credentialの形式が不正です".to_owned())?;
                let id = CredentialId::parse(stored.id)
                    .map_err(|_| "credential IDが不正です".to_owned())?;
                let mut secret = decode_hex(&stored.secret)?;
                let request = match workspace.clone() {
                    Some(workspace) => {
                        AuthenticateRequest::new(hello.clone(), workspace, &id, &secret)
                    }
                    None => AuthenticateRequest::new_auto(hello.clone(), &id, &secret),
                };
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
        match BrowserConnection::connect_with_hello(url, hello.clone(), auth).await {
            Ok(result) => result,
            Err(_) if can_retry_pair => {
                let token = token.ok_or_else(|| "pairing tokenが必要です".to_owned())?;
                let mut secret = decode_hex(token)?;
                let retry_hello = hello.clone();
                let request = PairRequest::new(hello, &secret);
                secret.zeroize();
                BrowserConnection::connect_with_hello(
                    url,
                    retry_hello,
                    ClientMessage::Pair(request),
                )
                .await?
            }
            Err(error) => return Err(error),
        };
    let (credential, selected_workspace) = credential_from_response(response)?;
    Ok((connection, credential, selected_workspace))
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
) -> Result<(Option<String>, Option<WorkspaceId>), String> {
    match response {
        ServerMessage::Paired(mut paired) => {
            let workspace_id = paired.hello.workspace_id.clone();
            let id = paired
                .credential_id()
                .map_err(|_| "credential IDが不正です".to_owned())?;
            let secret = paired
                .take_secret()
                .map_err(|_| "credential secretが不正です".to_owned())?;
            let stored = StoredCredential {
                id: id.as_str().to_owned(),
                secret: encode_hex(&secret),
            };
            Ok((
                Some(
                    serde_json::to_string(&stored)
                        .map_err(|_| "credentialを保存できません".to_owned())?,
                ),
                workspace_id,
            ))
        }
        ServerMessage::Authenticated(hello) => Ok((None, hello.workspace_id)),
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
                return Err(format!("認証が拒否されました: {}", error.code.as_str()));
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
        self.socket.set_onopen(None);
        self.socket.set_onmessage(None);
        self.socket.set_onclose(None);
        self.socket.set_onerror(None);
        self.shared.borrow_mut().fail("WebSocketが終了しました");
        let _ = self.socket.close();
    }
}

impl crate::AsyncHostConnection for BrowserConnection {
    async fn request(&self, body: RequestBody) -> Result<ResponseBody, String> {
        BrowserConnection::request(self, body).await
    }
}
