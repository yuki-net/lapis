use std::{collections::HashMap, sync::Mutex};

use lapis_client_api::{
    RequestBody, ResponseBody, WorkbenchGetRequest, WorkbenchPutRequest, WorkbenchPutResponse,
};
use lapis_editor_core::WorkspaceId;
use lapis_workbench::{
    ConversationViewState, VersionedViewState, WorkbenchConnection, WorkbenchSubscription,
    WorkbenchUpdate,
};

use crate::{BackendClient, client::Client};

pub(crate) struct HostWorkbench {
    host: BackendClient,
    clients: Mutex<HashMap<lapis_client_api::WorkspaceId, Client>>,
}

impl HostWorkbench {
    pub(crate) fn new(host: BackendClient) -> Self {
        Self {
            host,
            clients: Mutex::new(HashMap::new()),
        }
    }

    fn client(&self, workspace: &WorkspaceId) -> Result<Client, String> {
        let id = lapis_client_api::WorkspaceId::try_new(workspace.as_str())
            .map_err(|e| e.to_string())?;
        let mut clients = self
            .clients
            .lock()
            .map_err(|_| "Workbench接続を取得できません".to_owned())?;
        if let Some(client) = clients.get(&id) {
            return Ok(client.clone());
        }
        let client = self
            .host
            .connect_workspace(id.clone())
            .map_err(|e| e.to_string())?;
        clients.insert(id, client.clone());
        Ok(client)
    }
}

fn decode(snapshot: lapis_client_api::WorkbenchSnapshot) -> Result<VersionedViewState, String> {
    Ok(VersionedViewState {
        revision: snapshot.revision,
        view: serde_json::from_value(snapshot.view).map_err(|e| e.to_string())?,
    })
}

impl WorkbenchConnection for HostWorkbench {
    fn get(&self, workspace: &WorkspaceId) -> Result<VersionedViewState, String> {
        let client = self.client(workspace)?;
        match client
            .request(RequestBody::WorkbenchGet(WorkbenchGetRequest {
                workspace_id: client.workspace_id().clone(),
            }))
            .map_err(|e| e.to_string())?
        {
            ResponseBody::WorkbenchGet(snapshot) => decode(snapshot),
            ResponseBody::Error(error) => Err(error.code.as_str().to_owned()),
            _ => Err("Workbench応答が不正です".to_owned()),
        }
    }

    fn update(
        &self,
        workspace: &WorkspaceId,
        expected: u64,
        view: ConversationViewState,
    ) -> Result<WorkbenchUpdate, String> {
        let client = self.client(workspace)?;
        let view = serde_json::to_value(view).map_err(|e| e.to_string())?;
        match client
            .request(RequestBody::WorkbenchPut(WorkbenchPutRequest {
                workspace_id: client.workspace_id().clone(),
                expected_revision: expected,
                view,
            }))
            .map_err(|e| e.to_string())?
        {
            ResponseBody::WorkbenchPut(WorkbenchPutResponse::Updated(snapshot)) => {
                Ok(WorkbenchUpdate::Updated(decode(snapshot)?))
            }
            ResponseBody::WorkbenchPut(WorkbenchPutResponse::Conflict(snapshot)) => {
                Ok(WorkbenchUpdate::Conflict(decode(snapshot)?))
            }
            ResponseBody::Error(error) => Err(error.code.as_str().to_owned()),
            _ => Err("Workbench応答が不正です".to_owned()),
        }
    }

    fn subscribe(&self, workspace: &WorkspaceId) -> Result<Box<dyn WorkbenchSubscription>, String> {
        let id = lapis_client_api::WorkspaceId::try_new(workspace.as_str())
            .map_err(|e| e.to_string())?;
        let client = self.host.connect_workspace(id).map_err(|e| e.to_string())?;
        let receiver = client.subscribe().map_err(|e| e.to_string())?;
        Ok(Box::new(HostWorkbenchSubscription {
            _client: client,
            receiver,
            watermark: None,
        }))
    }
}

struct HostWorkbenchSubscription {
    _client: Client,
    receiver: lapis_backend_state::BackendEventReceiver,
    watermark: Option<u64>,
}

impl WorkbenchSubscription for HostWorkbenchSubscription {
    fn recv_timeout(&mut self, timeout: std::time::Duration) -> Result<Option<u64>, String> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let event = match self
                .receiver
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            {
                Ok(event) => event,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return Ok(None),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("Workbench購読が切断されました".to_owned());
                }
            };
            let sequence = event.event_sequence.value();
            if let Some(previous) = self.watermark {
                if sequence <= previous {
                    continue;
                }
                if previous.checked_add(1) != Some(sequence) {
                    return Err("Workbench通知が欠落しました".to_owned());
                }
            }
            self.watermark = Some(sequence);
            match event.body {
                lapis_client_api::EventBody::WorkbenchChanged { revision, .. } => {
                    return Ok(Some(revision));
                }
                lapis_client_api::EventBody::SnapshotRequired { .. } => {
                    return Err("Workbench状態の再取得が必要です".to_owned());
                }
                _ => {}
            }
        }
    }
}
