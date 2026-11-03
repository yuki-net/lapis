use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};

use futures_channel::{mpsc, oneshot};
use futures_util::future::{Either, select};
use gloo_timers::future::TimeoutFuture;
use lapis_client_api::{
    CURRENT_PROTOCOL, ClientMessage, EventEnvelope, RequestBody, ResponseBody, ServerMessage,
};
use lapis_client_core::ClientCore;
use wasm_bindgen::{JsCast, closure::Closure};
use web_sys::{Event, MessageEvent, WebSocket};

type Reply<T> = oneshot::Sender<Result<T, String>>;

struct Shared {
    core: ClientCore,
    opened: Option<Reply<()>>,
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
        let pairing = match &auth {
            ClientMessage::Pair(_) => true,
            ClientMessage::Authenticate(_) => false,
            _ => return Err("認証要求が必要です".into()),
        };
        let socket = WebSocket::new_with_str(url, lapis_wire::REMOTE_WEBSOCKET_PROTOCOL)
            .map_err(|_| "WebSocketを開始できません")?;
        let (opened, wait_open) = oneshot::channel();
        let (authentication, wait_auth) = oneshot::channel();
        let (events_tx, events) = mpsc::channel(256);
        let shared = Rc::new(RefCell::new(Shared {
            core: ClientCore::default(),
            opened: Some(opened),
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

fn handle_message(shared: &mut Shared, message: ServerMessage) -> Result<(), String> {
    if !shared.authenticated {
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

impl lapis_client_core::AsyncHostConnection for BrowserConnection {
    async fn request(&self, body: RequestBody) -> Result<ResponseBody, String> {
        BrowserConnection::request(self, body).await
    }
}
