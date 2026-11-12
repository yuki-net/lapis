//! Workspace view state の Session 共有を、Desktop の Client から Host へ橋渡しする。
//!
//! 契約の正本は runtime と protocol にあり、ここでは 1 つの Session で
//! subscribe / update / event 購読をそろえ、workspace ごとの接続を再利用する。

use std::{sync::Mutex, time::Duration};

use client_api::{
    EventBody, RequestBody, ResponseBody, WorkspaceViewSubscribeRequest,
    WorkspaceViewUnsubscribeRequest, WorkspaceViewUpdateRequest,
};
use editor_core::WorkspaceId;
use workbench::{
    SessionViewConnection, SessionViewEvent, SessionViewEvents, SessionViewSnapshot, WorkspaceView,
};

use crate::{BackendClient, client::Client};

pub(super) struct HostSessionView {
    host: BackendClient,
    client: Mutex<Option<(client_api::WorkspaceId, Client)>>,
}

impl HostSessionView {
    pub(super) fn new(host: BackendClient) -> Self {
        Self {
            host,
            client: Mutex::new(None),
        }
    }

    fn client(&self, workspace: &WorkspaceId) -> Result<(Client, client_api::WorkspaceId), String> {
        let id = client_api::WorkspaceId::try_from(workspace).map_err(|error| error.to_string())?;
        let mut slot = self
            .client
            .lock()
            .map_err(|_| "Workspace view 接続の lock に失敗しました".to_owned())?;
        if let Some((cached, client)) = slot.as_ref()
            && cached == &id
        {
            return Ok((client.clone(), id));
        }
        let client = self
            .host
            .connect_workspace(id.clone())
            .map_err(|error| error.to_string())?;
        *slot = Some((id.clone(), client.clone()));
        Ok((client, id))
    }
}

impl SessionViewConnection for HostSessionView {
    fn subscribe(&self, workspace: &WorkspaceId) -> Result<SessionViewSnapshot, String> {
        let (client, id) = self.client(workspace)?;
        match client
            .request(RequestBody::WorkspaceViewSubscribe(
                WorkspaceViewSubscribeRequest { workspace_id: id },
            ))
            .map_err(|error| error.to_string())?
        {
            ResponseBody::WorkspaceViewSubscribe(snapshot) => Ok(SessionViewSnapshot {
                revision: snapshot.revision,
                view: snapshot.view,
            }),
            ResponseBody::Error(error) => Err(error
                .detail
                .unwrap_or_else(|| error.code.as_str().to_owned())),
            _ => Err("Workspace view の購読応答が不正です".to_owned()),
        }
    }

    fn unsubscribe(&self, workspace: &WorkspaceId) -> Result<(), String> {
        let (client, id) = self.client(workspace)?;
        match client
            .request(RequestBody::WorkspaceViewUnsubscribe(
                WorkspaceViewUnsubscribeRequest { workspace_id: id },
            ))
            .map_err(|error| error.to_string())?
        {
            ResponseBody::WorkspaceViewUnsubscribe(_) => Ok(()),
            ResponseBody::Error(error) => Err(error
                .detail
                .unwrap_or_else(|| error.code.as_str().to_owned())),
            _ => Err("Workspace view の購読解除応答が不正です".to_owned()),
        }
    }

    fn update(&self, workspace: &WorkspaceId, view: WorkspaceView) -> Result<u64, String> {
        let (client, id) = self.client(workspace)?;
        match client
            .request(RequestBody::WorkspaceViewUpdate(
                WorkspaceViewUpdateRequest {
                    workspace_id: id,
                    view,
                },
            ))
            .map_err(|error| error.to_string())?
        {
            ResponseBody::WorkspaceViewUpdate(response) => Ok(response.revision),
            ResponseBody::Error(error) => Err(error
                .detail
                .unwrap_or_else(|| error.code.as_str().to_owned())),
            _ => Err("Workspace view の更新応答が不正です".to_owned()),
        }
    }

    fn events(&self, workspace: &WorkspaceId) -> Result<Box<dyn SessionViewEvents>, String> {
        let (client, id) = self.client(workspace)?;
        let receiver = client.subscribe().map_err(|error| error.to_string())?;
        Ok(Box::new(HostSessionViewEvents {
            _client: client,
            workspace: id,
            receiver,
            watermark: None,
        }))
    }
}

struct HostSessionViewEvents {
    _client: Client,
    workspace: client_api::WorkspaceId,
    receiver: backend_state::BackendEventReceiver,
    watermark: Option<u64>,
}

impl SessionViewEvents for HostSessionViewEvents {
    fn next(&mut self, timeout: Duration) -> Result<Option<SessionViewEvent>, String> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let event = match self
                .receiver
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            {
                Ok(event) => event,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return Ok(None),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("Workspace view の購読が切断されました".to_owned());
                }
            };
            // event は Workspace 単位の連番で届く。欠落は取りこぼしとして切断扱いにし、
            // snapshot で戻す。view の revision は参加者がいない間も進むため判定に使わない。
            let sequence = event.event_sequence.value();
            if let Some(previous) = self.watermark {
                if sequence <= previous {
                    continue;
                }
                if previous.checked_add(1) != Some(sequence) {
                    return Err("Workspace view の event が欠落しました".to_owned());
                }
            }
            self.watermark = Some(sequence);
            match event.body {
                EventBody::WorkspaceViewChanged {
                    workspace_id,
                    revision,
                    view,
                } if workspace_id == self.workspace => {
                    return Ok(Some(SessionViewEvent { revision, view }));
                }
                // 再接続後は snapshot で戻す。event の欠落をここで推測しない。
                EventBody::SnapshotRequired { .. } => {
                    return Err("Workspace view の再同期が必要です".to_owned());
                }
                _ => {}
            }
        }
    }
}
