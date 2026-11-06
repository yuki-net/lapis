//! DesktopとWebで共有するWorkbenchの非同期操作。

use client_api::{
    RequestBody, ResponseBody, WorkbenchGetRequest, WorkbenchPutRequest, WorkbenchPutResponse,
    WorkbenchSnapshot, WorkspaceId,
};
use workbench::{
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

    /// 接続を交換する前に正本を再取得し、未確定値と競合を保持する。
    pub async fn reconnect(
        &mut self,
        connection: &'a C,
    ) -> Result<Option<VersionedViewState>, String> {
        let workspace = self
            .workspace
            .as_ref()
            .ok_or("Workspaceが接続されていません")?;
        let snapshot = get(connection, workspace).await?;
        let refreshed = self.replica.refresh(snapshot)?;
        self.connection = connection;
        Ok(refreshed)
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

    /// 中断された送信を同じrevisionから再試行する。競合があれば上書きしない。
    pub async fn retry_pending(&mut self) -> Result<WorkbenchUpdate, String> {
        let pending = self
            .replica
            .pending()
            .cloned()
            .ok_or("未確定のWorkbench変更がありません")?;
        self.save(pending).await
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
    let view = client_api::encode_workbench_view(view).map_err(|error| error.to_string())?;
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

    struct ReconnectConnection {
        snapshot: WorkbenchSnapshot,
        hang_write: bool,
    }

    impl AsyncHostConnection for ReconnectConnection {
        async fn request(&self, body: RequestBody) -> Result<ResponseBody, String> {
            match body {
                RequestBody::WorkbenchGet(_) => {
                    Ok(ResponseBody::WorkbenchGet(self.snapshot.clone()))
                }
                RequestBody::WorkbenchPut(_) if self.hang_write => std::future::pending().await,
                RequestBody::WorkbenchPut(request) => Ok(ResponseBody::WorkbenchPut(
                    WorkbenchPutResponse::Updated(WorkbenchSnapshot {
                        revision: request.expected_revision + 1,
                        view: request.view,
                    }),
                )),
                _ => Err("unexpected request".into()),
            }
        }
    }

    #[test]
    fn reconnect_retains_pending_view_and_detects_a_remote_change() {
        let initial = ReconnectConnection {
            snapshot: WorkbenchSnapshot {
                revision: 0,
                view: serde_json::Value::Null,
            },
            hang_write: true,
        };
        let remote = ConversationViewState::default();
        let replacement = ReconnectConnection {
            snapshot: WorkbenchSnapshot {
                revision: 1,
                view: serde_json::to_value(&remote).unwrap(),
            },
            hang_write: false,
        };
        let mut sync = AsyncWorkbenchSync::new(&initial);
        let mut context = Context::from_waker(Waker::noop());
        {
            let mut open = std::pin::pin!(sync.open(WorkspaceId::try_new("workspace-1").unwrap()));
            assert!(matches!(
                open.as_mut().poll(&mut context),
                Poll::Ready(Ok(_))
            ));
        }
        let local = ConversationViewState {
            show_panel_controls: false,
            ..Default::default()
        };
        {
            let mut save = std::pin::pin!(sync.save(local.clone()));
            assert!(save.as_mut().poll(&mut context).is_pending());
        }
        {
            let mut reconnect = std::pin::pin!(sync.reconnect(&replacement));
            assert!(matches!(
                reconnect.as_mut().poll(&mut context),
                Poll::Ready(Ok(None))
            ));
        }
        assert_eq!(sync.pending(), Some(&local));
        assert_eq!(sync.conflict().unwrap().view, Some(remote));
        {
            let mut retry = std::pin::pin!(sync.retry_pending());
            assert!(matches!(
                retry.as_mut().poll(&mut context),
                Poll::Ready(Ok(WorkbenchUpdate::Conflict(_)))
            ));
        }
        assert_eq!(sync.pending(), Some(&local));
        {
            let mut retry = std::pin::pin!(sync.retry_local());
            assert!(matches!(
                retry.as_mut().poll(&mut context),
                Poll::Ready(Ok(WorkbenchUpdate::Updated(_)))
            ));
        }
        assert!(sync.pending().is_none());
        assert_eq!(sync.confirmed().unwrap().view, Some(local));
    }

    #[test]
    fn reconnect_can_retry_a_pending_write_without_a_remote_change() {
        let snapshot = WorkbenchSnapshot {
            revision: 0,
            view: serde_json::Value::Null,
        };
        let initial = ReconnectConnection {
            snapshot: snapshot.clone(),
            hang_write: true,
        };
        let replacement = ReconnectConnection {
            snapshot,
            hang_write: false,
        };
        let mut sync = AsyncWorkbenchSync::new(&initial);
        let mut context = Context::from_waker(Waker::noop());
        {
            let mut open = std::pin::pin!(sync.open(WorkspaceId::try_new("workspace-1").unwrap()));
            assert!(matches!(
                open.as_mut().poll(&mut context),
                Poll::Ready(Ok(_))
            ));
        }
        let local = ConversationViewState::default();
        {
            let mut save = std::pin::pin!(sync.save(local.clone()));
            assert!(save.as_mut().poll(&mut context).is_pending());
        }
        {
            let mut reconnect = std::pin::pin!(sync.reconnect(&replacement));
            assert!(matches!(
                reconnect.as_mut().poll(&mut context),
                Poll::Ready(Ok(None))
            ));
        }
        {
            let mut retry = std::pin::pin!(sync.retry_pending());
            assert!(matches!(
                retry.as_mut().poll(&mut context),
                Poll::Ready(Ok(WorkbenchUpdate::Updated(_)))
            ));
        }
        assert!(sync.pending().is_none());
        assert_eq!(sync.confirmed().unwrap().view, Some(local));
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
