//! ページから使う接続とWorkbench操作。通信形式はDesktopと同じ共通Clientを使う。

use std::rc::Rc;

use futures_channel::mpsc;
use futures_util::StreamExt;
use lapis_client_api::{
    AuthenticateRequest, CURRENT_PROTOCOL, CapabilityId, CapabilitySet, ClientHello, ClientId,
    ClientKind, ClientMessage, CredentialId, EventBody, EventEnvelope, PairRequest, ProtocolRange,
    RequestBody, ServerMessage, WorkspaceConnectRequest, WorkspaceId,
};
use lapis_client_core::{
    AsyncHostConnection,
    workbench::{get, update},
};
use lapis_workbench::{ConversationViewState, WorkbenchReplica, WorkbenchWritePlan};
use wasm_bindgen::prelude::*;
use zeroize::Zeroize;

use crate::connection::BrowserConnection;

const CAPABILITIES: &[&str] = &[
    "workspace.list",
    "workspace.connect",
    "workspace.sync",
    "workspace.workbench.write",
    "workspace.documents.read",
    "workspace.documents.write",
];

/// ページが保持するcredential。secretはlocalStorageへ保存する前提でhex文字列にする。
#[derive(serde::Deserialize, serde::Serialize)]
struct StoredCredential {
    id: String,
    secret: String,
}

#[wasm_bindgen]
pub struct WebSession {
    connection: Rc<BrowserConnection>,
    workspace: WorkspaceId,
    replica: WorkbenchReplica,
    events: Option<mpsc::Receiver<EventEnvelope>>,
    credential: Option<String>,
}

#[wasm_bindgen]
impl WebSession {
    /// pairing tokenまたは保存済みcredentialで接続し、Workbenchの正本を取得する。
    #[wasm_bindgen(js_name = connect)]
    pub async fn connect(
        url: String,
        client_id: String,
        workspace: String,
        token: Option<String>,
        credential: Option<String>,
    ) -> Result<WebSession, JsValue> {
        let workspace = WorkspaceId::try_new(workspace)
            .map_err(|_| JsValue::from_str("Workspace IDが不正です"))?;
        let hello = hello(&client_id)?;
        let auth = match credential {
            Some(credential) => {
                let stored: StoredCredential = serde_json::from_str(&credential)
                    .map_err(|_| JsValue::from_str("保存済みcredentialが不正です"))?;
                let id = CredentialId::parse(stored.id)
                    .map_err(|_| JsValue::from_str("credential IDが不正です"))?;
                let mut secret = decode_hex(&stored.secret)?;
                let request = AuthenticateRequest::new(hello, workspace.clone(), &id, &secret);
                secret.zeroize();
                ClientMessage::Authenticate(request)
            }
            None => {
                let token = token.ok_or_else(|| JsValue::from_str("pairing tokenが必要です"))?;
                let mut secret = decode_hex(&token)?;
                let request = PairRequest::new(hello, &secret);
                secret.zeroize();
                ClientMessage::Pair(request)
            }
        };
        let (connection, response) = BrowserConnection::connect(&url, auth)
            .await
            .map_err(|error| JsValue::from_str(&error))?;
        let mut connection = Rc::new(connection);
        let events = Rc::get_mut(&mut connection)
            .ok_or_else(|| JsValue::from_str("接続を初期化できません"))?
            .take_events()
            .ok_or_else(|| JsValue::from_str("イベント購読を開始できません"))?;
        let mut credential = None;
        match response {
            ServerMessage::Paired(mut paired) => {
                let id = paired
                    .credential_id()
                    .map_err(|_| JsValue::from_str("credential IDが不正です"))?;
                let secret = paired
                    .take_secret()
                    .map_err(|_| JsValue::from_str("credential secretが不正です"))?;
                credential = Some(
                    serde_json::to_string(&StoredCredential {
                        id: id.as_str().to_owned(),
                        secret: encode_hex(&secret),
                    })
                    .map_err(|_| JsValue::from_str("credentialを保存できません"))?,
                );
            }
            ServerMessage::Authenticated(_) => {}
            _ => return Err(JsValue::from_str("認証応答が不正です")),
        }
        match AsyncHostConnection::request(
            &*connection,
            RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
                workspace_id: workspace.clone(),
            }),
        )
        .await
        {
            Ok(_) => {}
            Err(error) => return Err(JsValue::from_str(&error)),
        }
        let snapshot = get(&*connection, &workspace)
            .await
            .map_err(|error| JsValue::from_str(&error))?;
        let mut replica = WorkbenchReplica::default();
        replica
            .open(snapshot)
            .map_err(|error| JsValue::from_str(&error))?;
        Ok(Self {
            connection,
            workspace,
            replica,
            events: Some(events),
            credential,
        })
    }

    /// pairing直後だけ返る。ページはlocalStorageへ保存し、次回以降の認証に使う。
    #[wasm_bindgen(js_name = takeCredential)]
    pub fn take_credential(&mut self) -> Option<String> {
        self.credential.take()
    }

    /// 現在の正本・未確定値・競合をJSONで返す。
    #[wasm_bindgen(js_name = stateJson)]
    pub fn state_json(&self) -> Result<String, JsValue> {
        let confirmed = self.replica.confirmed().cloned().unwrap_or_default();
        serde_json::to_string(&serde_json::json!({
            "revision": confirmed.revision,
            "view": confirmed.view,
            "pending": self.replica.pending().is_some(),
            "conflict": self.replica.conflict(),
        }))
        .map_err(|_| JsValue::from_str("状態をJSONにできません"))
    }

    /// 変更をHostへ送る。競合時は未確定値を保持したまま競合状態を返す。
    #[wasm_bindgen(js_name = saveJson)]
    pub async fn save_json(&mut self, view_json: String) -> Result<String, JsValue> {
        let view: ConversationViewState = serde_json::from_str(&view_json)
            .map_err(|_| JsValue::from_str("Workbenchの形式が不正です"))?;
        let plan = self
            .replica
            .stage(view)
            .map_err(|error| JsValue::from_str(&error))?;
        if let WorkbenchWritePlan::Update { revision, view } = plan {
            let result = update(&*self.connection, &self.workspace, revision, &view)
                .await
                .map_err(|error| JsValue::from_str(&error))?;
            self.replica
                .finish(result)
                .map_err(|error| JsValue::from_str(&error))?;
        }
        self.state_json()
    }

    /// Hostの状態を採用し、未確定値と競合を捨てる。
    #[wasm_bindgen(js_name = acceptRemoteJson)]
    pub fn accept_remote_json(&mut self) -> Result<String, JsValue> {
        self.replica
            .accept_remote()
            .map_err(|error| JsValue::from_str(&error))?;
        self.state_json()
    }

    /// 競合を残したまま自分の未確定値を再送する。
    #[wasm_bindgen(js_name = retryLocalJson)]
    pub async fn retry_local_json(&mut self) -> Result<String, JsValue> {
        let pending = self
            .replica
            .prepare_retry()
            .map_err(|error| JsValue::from_str(&error))?;
        let plan = self
            .replica
            .stage(pending)
            .map_err(|error| JsValue::from_str(&error))?;
        if let WorkbenchWritePlan::Update { revision, view } = plan {
            let result = update(&*self.connection, &self.workspace, revision, &view)
                .await
                .map_err(|error| JsValue::from_str(&error))?;
            self.replica
                .finish(result)
                .map_err(|error| JsValue::from_str(&error))?;
        }
        self.state_json()
    }

    /// 他のClientの変更通知を購読する。通知後はページがrefreshJsonで正本を取り直す。
    #[wasm_bindgen(js_name = startEvents)]
    pub fn start_events(&mut self, callback: js_sys::Function) -> Result<(), JsValue> {
        let mut events = self
            .events
            .take()
            .ok_or_else(|| JsValue::from_str("イベント購読がありません"))?;
        wasm_bindgen_futures::spawn_local(async move {
            while let Some(event) = events.next().await {
                if matches!(event.body, EventBody::WorkbenchChanged { .. })
                    && callback.call0(&JsValue::NULL).is_err()
                {
                    return;
                }
            }
            let _ = callback.call0(&JsValue::NULL);
        });
        Ok(())
    }

    /// Hostから正本を取り直す。未確定値と競合は失わない。
    #[wasm_bindgen(js_name = refreshJson)]
    pub async fn refresh_json(&mut self) -> Result<String, JsValue> {
        let snapshot = get(&*self.connection, &self.workspace)
            .await
            .map_err(|error| JsValue::from_str(&error))?;
        self.replica
            .refresh(snapshot)
            .map_err(|error| JsValue::from_str(&error))?;
        self.state_json()
    }
}

fn hello(client_id: &str) -> Result<ClientHello, JsValue> {
    let capabilities = CAPABILITIES
        .iter()
        .map(|capability| {
            CapabilityId::try_new(*capability)
                .map_err(|_| JsValue::from_str("capabilityが不正です"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ClientHello {
        protocol: ProtocolRange::exact(CURRENT_PROTOCOL),
        client_id: ClientId::try_new(client_id)
            .map_err(|_| JsValue::from_str("Client IDが不正です"))?,
        client_name: "lapis-web".into(),
        client_kind: ClientKind::Desktop,
        requested_capabilities: CapabilitySet::try_new(capabilities)
            .map_err(|_| JsValue::from_str("capabilityが不正です"))?,
    })
}

fn decode_hex(value: &str) -> Result<[u8; 32], JsValue> {
    let value = value.trim();
    if value.len() != 64 {
        return Err(JsValue::from_str("secretの長さが不正です"));
    }
    let mut output = [0u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let digit = |byte: u8| match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            _ => None,
        };
        let high = digit(pair[0]).ok_or_else(|| JsValue::from_str("secretが不正です"))?;
        let low = digit(pair[1]).ok_or_else(|| JsValue::from_str("secretが不正です"))?;
        output[index] = (high << 4) | low;
    }
    Ok(output)
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
