//! DesktopとWebで共有するWorkbenchの非同期操作。

use lapis_client_api::{
    RequestBody, ResponseBody, WorkbenchGetRequest, WorkbenchPutRequest, WorkbenchPutResponse,
    WorkbenchSnapshot, WorkspaceId,
};
use lapis_workbench::{
    ConversationViewState, VersionedViewState, WorkbenchReplica, WorkbenchUpdate,
    WorkbenchWritePlan,
};

use crate::AsyncHostConnection;

/// 両Clientで同じ状態遷移を使い、通信失敗やfutureの中断時も未確定値を保持する。
pub struct AsyncWorkbenchSync<'a, C: AsyncHostConnection> {
    connection: &'a C,
    workspace: Option<WorkspaceId>,
    replica: WorkbenchReplica,
}

impl<'a, C: AsyncHostConnection> AsyncWorkbenchSync<'a, C> {
    pub fn new(connection: &'a C) -> Self {
        Self {
            connection,
            workspace: None,
            replica: WorkbenchReplica::default(),
        }
    }

    pub fn pending(&self) -> Option<&ConversationViewState> {
        self.replica.pending()
    }
    pub fn conflict(&self) -> Option<&VersionedViewState> {
        self.replica.conflict()
    }
    pub fn confirmed(&self) -> Option<&VersionedViewState> {
        self.replica.confirmed()
    }

    pub async fn open(&mut self, workspace: WorkspaceId) -> Result<VersionedViewState, String> {
        self.replica.ensure_can_open()?;
        let snapshot = get(self.connection, &workspace).await?;
        self.replica.open(snapshot.clone())?;
        self.workspace = Some(workspace);
        Ok(snapshot)
    }

    pub async fn refresh(&mut self) -> Result<Option<VersionedViewState>, String> {
        let workspace = self
            .workspace
            .as_ref()
            .ok_or("Workspaceが接続されていません")?;
        let snapshot = get(self.connection, workspace).await?;
        self.replica.refresh(snapshot)
    }

    pub async fn save(&mut self, view: ConversationViewState) -> Result<WorkbenchUpdate, String> {
        let plan = self.replica.stage(view)?;
        let workspace = self
            .workspace
            .as_ref()
            .ok_or("Workspaceが接続されていません")?;
        let result = match plan {
            WorkbenchWritePlan::Conflict(snapshot) => {
                return Ok(WorkbenchUpdate::Conflict(snapshot));
            }
            WorkbenchWritePlan::Update { revision, view } => {
                update(self.connection, workspace, revision, &view).await?
            }
        };
        self.replica.finish(result)
    }

    pub fn accept_remote(&mut self) -> Result<VersionedViewState, String> {
        self.replica.accept_remote()
    }

    pub async fn retry_local(&mut self) -> Result<WorkbenchUpdate, String> {
        let pending = self.replica.prepare_retry()?;
        self.save(pending).await
    }
}

fn decode(snapshot: WorkbenchSnapshot) -> Result<VersionedViewState, String> {
    Ok(VersionedViewState {
        revision: snapshot.revision,
        view: serde_json::from_value(snapshot.view).map_err(|error| error.to_string())?,
    })
}

pub async fn get(
    connection: &impl AsyncHostConnection,
    workspace: &WorkspaceId,
) -> Result<VersionedViewState, String> {
    match connection
        .request(RequestBody::WorkbenchGet(WorkbenchGetRequest {
            workspace_id: workspace.clone(),
        }))
        .await?
    {
        ResponseBody::WorkbenchGet(snapshot) => decode(snapshot),
        ResponseBody::Error(error) => Err(error.code.as_str().to_owned()),
        _ => Err("Workbench応答が不正です".to_owned()),
    }
}

pub async fn update(
    connection: &impl AsyncHostConnection,
    workspace: &WorkspaceId,
    expected: u64,
    view: &ConversationViewState,
) -> Result<WorkbenchUpdate, String> {
    let view = serde_json::to_value(view).map_err(|error| error.to_string())?;
    match connection
        .request(RequestBody::WorkbenchPut(WorkbenchPutRequest {
            workspace_id: workspace.clone(),
            expected_revision: expected,
            view,
        }))
        .await?
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        future::Future,
        task::{Context, Poll, Waker},
    };

    struct PendingConnection;
    impl AsyncHostConnection for PendingConnection {
        async fn request(&self, body: RequestBody) -> Result<ResponseBody, String> {
            match body {
                RequestBody::WorkbenchGet(_) => Ok(ResponseBody::WorkbenchGet(WorkbenchSnapshot {
                    revision: 0,
                    view: serde_json::Value::Null,
                })),
                RequestBody::WorkbenchPut(_) => std::future::pending().await,
                _ => Err("unexpected request".into()),
            }
        }
    }

    #[test]
    fn cancelling_an_in_flight_save_retains_the_local_view() {
        let connection = PendingConnection;
        let mut sync = AsyncWorkbenchSync::new(&connection);
        let workspace = WorkspaceId::try_new("workspace-1").unwrap();
        let mut context = Context::from_waker(Waker::noop());
        {
            let mut open = std::pin::pin!(sync.open(workspace));
            assert!(matches!(
                open.as_mut().poll(&mut context),
                Poll::Ready(Ok(_))
            ));
        }
        let view = ConversationViewState {
            show_panel_controls: false,
            ..Default::default()
        };
        {
            let save = sync.save(view.clone());
            let mut save = std::pin::pin!(save);
            assert!(save.as_mut().poll(&mut context).is_pending());
        }
        assert_eq!(sync.pending(), Some(&view));
        let mut reopen = std::pin::pin!(sync.open(WorkspaceId::try_new("other").unwrap()));
        assert!(matches!(
            reopen.as_mut().poll(&mut context),
            Poll::Ready(Err(_))
        ));
    }
}
