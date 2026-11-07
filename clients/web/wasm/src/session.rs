//! ページから使う接続とWorkbench操作。通信形式はDesktopと同じ共通Clientを使う。

use std::rc::Rc;

use client_api::{
    ClientKind, EventBody, EventEnvelope, RequestBody, WorkspaceConnectRequest, WorkspaceId,
};
use client_core::{
    AsyncHostConnection,
    workbench::{get, update},
};
use futures_channel::mpsc;
use futures_util::StreamExt;
use wasm_bindgen::prelude::*;
use workbench::{ConversationViewState, WorkbenchReplica, WorkbenchWritePlan};

use crate::connection::BrowserConnection;

const CAPABILITIES: &[&str] = &[
    "workspace.list",
    "workspace.connect",
    "workspace.sync",
    "workspace.workbench.write",
    "workspace.documents.read",
    "workspace.documents.write",
];

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
        let (connection, credential, _) = client_core::browser::connect_web_client(
            client_core::browser::WebClientConnectOptions {
                url: &url,
                workspace: Some(workspace.clone()),
                client_id: &client_id,
                client_name: "lapis-web-test",
                client_kind: ClientKind::Desktop,
                token: token.as_deref(),
                credential: credential.as_deref(),
                id_token: None,
                capabilities: CAPABILITIES,
            },
        )
        .await
        .map_err(|error| JsValue::from_str(&error))?;
        let mut connection = Rc::new(connection);
        let events = Rc::get_mut(&mut connection)
            .ok_or_else(|| JsValue::from_str("接続を初期化できません"))?
            .take_events()
            .ok_or_else(|| JsValue::from_str("イベント購読を開始できません"))?;
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
