//! DesktopとWebで共通に使う非同期要求の順序と接続寿命。

use std::collections::HashMap;

use futures_channel::oneshot;
use lapis_client_api::{
    ClientMessage, EventEnvelope, EventSequence, RequestBody, RequestEnvelope, RequestId,
    ResponseBody, ServerMessage,
};

pub mod workbench;

const MAX_PENDING: usize = 64;

/// DesktopとWebがHostへ非同期要求を送るための共通境界。
/// futureの実行場所は各platformが決める。
pub trait AsyncHostConnection {
    fn request(
        &self,
        body: RequestBody,
    ) -> impl std::future::Future<Output = Result<ResponseBody, String>>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientError {
    Disconnected,
    TooManyRequests,
    CounterOverflow,
    Protocol,
    EventGap,
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Disconnected => "Hostに接続されていません",
            Self::TooManyRequests => "Hostへの未完了要求が上限に達しました",
            Self::CounterOverflow => "Clientの要求番号が上限に達しました",
            Self::Protocol => "Host応答が要求と一致しません",
            Self::EventGap => "Hostからのイベントが欠落しました",
        })
    }
}

impl std::error::Error for ClientError {}

struct Pending {
    request: RequestEnvelope,
    respond: oneshot::Sender<Result<ResponseBody, ClientError>>,
}

/// Socketの処理は各platformが所有し、この型は要求の照合と失敗を管理する。
#[derive(Default)]
pub struct ClientCore {
    generation: u64,
    next_request: u64,
    connected: bool,
    pending: HashMap<RequestId, Pending>,
    event_sequence: Option<EventSequence>,
}

impl ClientCore {
    /// 認証が完了してから新しい接続世代を開始する。
    pub fn connected(&mut self) -> Result<(), ClientError> {
        self.disconnect();
        self.generation = self
            .generation
            .checked_add(1)
            .ok_or(ClientError::CounterOverflow)?;
        self.next_request = 0;
        self.connected = true;
        Ok(())
    }

    pub fn begin(
        &mut self,
        body: RequestBody,
    ) -> Result<
        (
            ClientMessage,
            oneshot::Receiver<Result<ResponseBody, ClientError>>,
        ),
        ClientError,
    > {
        if !self.connected {
            return Err(ClientError::Disconnected);
        }
        if self.pending.len() >= MAX_PENDING {
            return Err(ClientError::TooManyRequests);
        }
        self.next_request = self
            .next_request
            .checked_add(1)
            .ok_or(ClientError::CounterOverflow)?;
        let request_id =
            RequestId::try_new(format!("client-{}-{}", self.generation, self.next_request))
                .map_err(|_| ClientError::CounterOverflow)?;
        let request = RequestEnvelope {
            request_id: request_id.clone(),
            body,
        };
        let (respond, receive) = oneshot::channel();
        self.pending.insert(
            request_id,
            Pending {
                request: request.clone(),
                respond,
            },
        );
        Ok((ClientMessage::Request(request), receive))
    }

    /// snapshot取得後は、そのwatermark以下のイベントを適用しない。
    pub fn set_watermark(&mut self, watermark: EventSequence) {
        self.event_sequence = Some(
            self.event_sequence
                .map_or(watermark, |current| current.max(watermark)),
        );
    }

    pub fn receive(
        &mut self,
        message: ServerMessage,
    ) -> Result<Option<EventEnvelope>, ClientError> {
        if !self.connected {
            return Err(ClientError::Disconnected);
        }
        match message {
            ServerMessage::Response(response) => {
                let Some(pending) = self.pending.remove(&response.request_id) else {
                    self.disconnect();
                    return Err(ClientError::Protocol);
                };
                if !response.matches_request(&pending.request) {
                    let _ = pending.respond.send(Err(ClientError::Protocol));
                    self.disconnect();
                    return Err(ClientError::Protocol);
                }
                if matches!(
                    (&pending.request.body, &response.body),
                    (RequestBody::SnapshotResync(request), ResponseBody::SnapshotResync(response))
                        if request.workspace_id != response.snapshot.workspace.workspace_id
                ) {
                    let _ = pending.respond.send(Err(ClientError::Protocol));
                    self.disconnect();
                    return Err(ClientError::Protocol);
                }
                if let ResponseBody::SnapshotResync(snapshot) = &response.body {
                    self.set_watermark(snapshot.snapshot.event_watermark);
                }
                let _ = pending.respond.send(Ok(response.body));
                Ok(None)
            }
            ServerMessage::Event(event) => {
                if let Some(previous) = self.event_sequence {
                    if event.event_sequence <= previous {
                        return Ok(None);
                    }
                    if previous.checked_next() != Some(event.event_sequence) {
                        self.disconnect();
                        return Err(ClientError::EventGap);
                    }
                }
                self.event_sequence = Some(event.event_sequence);
                Ok(Some(event))
            }
            _ => {
                self.disconnect();
                Err(ClientError::Protocol)
            }
        }
    }

    /// 送信結果が不明な要求は失敗させ、再接続しても自動再送しない。
    pub fn disconnect(&mut self) {
        self.connected = false;
        self.event_sequence = None;
        for (_, pending) in self.pending.drain() {
            let _ = pending.respond.send(Err(ClientError::Disconnected));
        }
    }
}

impl Drop for ClientCore {
    fn drop(&mut self) {
        self.disconnect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lapis_client_api::{
        EventBody, ResponseEnvelope, WorkspaceId, WorkspaceListRequest, WorkspaceListResponse,
    };

    #[test]
    fn disconnect_fails_pending_requests_without_reusing_identifiers() {
        let mut client = ClientCore::default();
        client.connected().unwrap();
        let (ClientMessage::Request(first), mut receive) = client
            .begin(RequestBody::WorkspaceList(WorkspaceListRequest::default()))
            .unwrap()
        else {
            panic!("request")
        };
        client.disconnect();
        assert!(matches!(
            receive.try_recv(),
            Ok(Some(Err(ClientError::Disconnected)))
        ));
        client.connected().unwrap();
        let (ClientMessage::Request(second), _) = client
            .begin(RequestBody::WorkspaceList(WorkspaceListRequest::default()))
            .unwrap()
        else {
            panic!("request")
        };
        assert_ne!(first.request_id, second.request_id);
        assert_eq!(client.pending.len(), 1);
    }

    #[test]
    fn pending_limit_is_distinct_from_disconnection() {
        let mut client = ClientCore::default();
        client.connected().unwrap();
        let mut replies = Vec::new();
        for _ in 0..MAX_PENDING {
            let (_, reply) = client
                .begin(RequestBody::WorkspaceList(WorkspaceListRequest::default()))
                .unwrap();
            replies.push(reply);
        }
        assert!(matches!(
            client.begin(RequestBody::WorkspaceList(WorkspaceListRequest::default())),
            Err(ClientError::TooManyRequests)
        ));
        assert_ne!(
            ClientError::TooManyRequests.to_string(),
            ClientError::Disconnected.to_string()
        );
    }

    #[test]
    fn response_is_matched_to_request_and_delivered() {
        let mut client = ClientCore::default();
        client.connected().unwrap();
        let (ClientMessage::Request(request), mut receive) = client
            .begin(RequestBody::WorkspaceList(WorkspaceListRequest::default()))
            .unwrap()
        else {
            panic!("request")
        };
        client
            .receive(ServerMessage::Response(ResponseEnvelope {
                request_id: request.request_id,
                body: ResponseBody::WorkspaceList(WorkspaceListResponse { workspaces: vec![] }),
            }))
            .unwrap();
        assert!(matches!(
            receive.try_recv(),
            Ok(Some(Ok(ResponseBody::WorkspaceList(_))))
        ));
    }

    #[test]
    fn event_gap_fails_pending_requests_and_disconnects() {
        let mut client = ClientCore::default();
        client.connected().unwrap();
        client.set_watermark(EventSequence::new(3));
        let (_, mut receive) = client
            .begin(RequestBody::WorkspaceList(WorkspaceListRequest::default()))
            .unwrap();
        let event = EventEnvelope {
            event_sequence: EventSequence::new(5),
            body: EventBody::WorkspaceChanged {
                workspace_id: WorkspaceId::try_new("workspace-1").unwrap(),
            },
        };
        assert!(matches!(
            client.receive(ServerMessage::Event(event)),
            Err(ClientError::EventGap)
        ));
        assert!(matches!(
            receive.try_recv(),
            Ok(Some(Err(ClientError::Disconnected)))
        ));
    }

    #[test]
    fn snapshot_watermark_is_installed_before_the_waiter_is_polled() {
        use lapis_client_api::{
            SnapshotReason, SnapshotRequest, SnapshotResponse, WorkspaceSnapshot, WorkspaceSummary,
        };

        let workspace_id = WorkspaceId::try_new("workspace-1").unwrap();
        let mut client = ClientCore::default();
        client.connected().unwrap();
        let (ClientMessage::Request(request), _unpolled_reply) = client
            .begin(RequestBody::SnapshotResync(SnapshotRequest {
                workspace_id: workspace_id.clone(),
                reason: SnapshotReason::Initial,
            }))
            .unwrap()
        else {
            panic!("request")
        };
        client
            .receive(ServerMessage::Response(ResponseEnvelope {
                request_id: request.request_id,
                body: ResponseBody::SnapshotResync(SnapshotResponse {
                    snapshot: WorkspaceSnapshot {
                        event_watermark: EventSequence::new(5),
                        workspace: WorkspaceSummary {
                            workspace_id: workspace_id.clone(),
                            name: "test".into(),
                        },
                        documents: vec![],
                        terminals: vec![],
                        git_repositories: vec![],
                    },
                }),
            }))
            .unwrap();
        let event = |sequence| {
            ServerMessage::Event(EventEnvelope {
                event_sequence: EventSequence::new(sequence),
                body: EventBody::WorkspaceChanged {
                    workspace_id: workspace_id.clone(),
                },
            })
        };
        assert!(client.receive(event(5)).unwrap().is_none());
        assert!(client.receive(event(6)).unwrap().is_some());
    }

    #[test]
    fn snapshot_for_another_workspace_is_rejected_before_advancing_watermark() {
        use lapis_client_api::{
            SnapshotReason, SnapshotRequest, SnapshotResponse, WorkspaceSnapshot, WorkspaceSummary,
        };
        let mut client = ClientCore::default();
        client.connected().unwrap();
        let (ClientMessage::Request(request), mut reply) = client
            .begin(RequestBody::SnapshotResync(SnapshotRequest {
                workspace_id: WorkspaceId::try_new("workspace-1").unwrap(),
                reason: SnapshotReason::Initial,
            }))
            .unwrap()
        else {
            panic!("request")
        };
        let result = client.receive(ServerMessage::Response(ResponseEnvelope {
            request_id: request.request_id,
            body: ResponseBody::SnapshotResync(SnapshotResponse {
                snapshot: WorkspaceSnapshot {
                    event_watermark: EventSequence::new(500),
                    workspace: WorkspaceSummary {
                        workspace_id: WorkspaceId::try_new("workspace-2").unwrap(),
                        name: "other".into(),
                    },
                    documents: vec![],
                    terminals: vec![],
                    git_repositories: vec![],
                },
            }),
        }));
        assert!(matches!(result, Err(ClientError::Protocol)));
        assert!(matches!(
            reply.try_recv(),
            Ok(Some(Err(ClientError::Protocol)))
        ));
        assert!(client.event_sequence.is_none());
    }
}
