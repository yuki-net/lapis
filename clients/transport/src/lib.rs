//! DesktopとWebで共通に使う非同期要求の順序と接続寿命。

use std::collections::HashMap;

use client_api::{
    ClientMessage, EventEnvelope, EventSequence, RequestBody, RequestEnvelope, RequestId,
    ResponseBody, ServerMessage,
};
use futures_channel::oneshot;

pub mod pkce;
pub mod web_capabilities;
pub mod workbench;

#[cfg(target_arch = "wasm32")]
pub mod browser;

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
    /// 接続中の Workspace。イベント連番の系列が同じかを判定する。
    workspace: Option<String>,
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

    /// Workspace へ接続したらイベント連番の系列が変わるため、基準を取り直す。
    fn reset_watermark(&mut self) {
        self.event_sequence = None;
    }

    /// Workspace へ接続した結果を反映する。
    ///
    /// イベント連番は Workspace ごとに独立している。接続中の Workspace へ接続し直す
    /// 場合は同じ系列が続くので基準を維持し、系列が変わる場合だけ取り直す。
    fn track_workspace(&mut self, workspace: &str, switched: bool) {
        if switched || self.workspace.as_deref() != Some(workspace) {
            self.reset_watermark();
        }
        self.workspace = Some(workspace.to_owned());
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
                // Workspace へ接続した直後の連番の扱いを決める。切り替えた場合と、
                // close などで購読が一度切れた場合は基準を取り直す。
                match (&pending.request.body, &response.body) {
                    (
                        RequestBody::WorkspaceConnect(_),
                        ResponseBody::WorkspaceConnect(connected),
                    ) => {
                        let switched = self.workspace.as_deref()
                            != Some(connected.workspace.workspace_id.as_str());
                        self.track_workspace(connected.workspace.workspace_id.as_str(), switched);
                    }
                    (RequestBody::WorkspaceClose(_), ResponseBody::WorkspaceClose(_)) => {
                        // 次に接続し直す Workspace の系列は close 中に進み得る。
                        self.workspace = None;
                        self.reset_watermark();
                    }
                    _ => {}
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
        self.workspace = None;
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
    use client_api::{
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

    /// Workspace へ接続する応答を返す。
    fn connected_to(client: &mut ClientCore, workspace: &WorkspaceId) {
        let (ClientMessage::Request(request), _reply) = client
            .begin(RequestBody::WorkspaceConnect(
                client_api::WorkspaceConnectRequest {
                    workspace_id: workspace.clone(),
                },
            ))
            .unwrap()
        else {
            panic!("request")
        };
        client
            .receive(ServerMessage::Response(ResponseEnvelope {
                request_id: request.request_id,
                body: ResponseBody::WorkspaceConnect(client_api::WorkspaceConnectResponse {
                    workspace: client_api::WorkspaceSummary {
                        workspace_id: workspace.clone(),
                        name: workspace.as_str().to_owned(),
                    },
                    identity: client_api::WorkspaceIdentitySnapshot {
                        workspace_id: workspace.clone(),
                        name: workspace.as_str().to_owned(),
                        root_name: workspace.as_str().to_owned(),
                    },
                }),
            }))
            .unwrap();
    }

    fn workspace_event(workspace: &WorkspaceId, sequence: u64) -> ServerMessage {
        ServerMessage::Event(EventEnvelope {
            event_sequence: EventSequence::new(sequence),
            body: EventBody::WorkspaceChanged {
                workspace_id: workspace.clone(),
            },
        })
    }

    #[test]
    fn reconnecting_to_the_same_workspace_keeps_event_gap_detection() {
        let first = WorkspaceId::try_new("workspace-1").unwrap();
        let mut client = ClientCore::default();
        client.connected().unwrap();
        connected_to(&mut client, &first);
        client.set_watermark(EventSequence::new(100));

        // 同じ Workspace へ接続し直しても系列は続く。欠番は検出したままにする。
        connected_to(&mut client, &first);
        assert!(matches!(
            client.receive(workspace_event(&first, 103)),
            Err(ClientError::EventGap)
        ));
    }

    #[test]
    fn switching_or_reconnecting_after_close_rebases_the_event_sequence() {
        let first = WorkspaceId::try_new("workspace-1").unwrap();
        let other = WorkspaceId::try_new("workspace-2").unwrap();
        let mut client = ClientCore::default();
        client.connected().unwrap();
        connected_to(&mut client, &first);
        client.set_watermark(EventSequence::new(100));

        // 別 Workspace へ切り替えると系列が変わるため、1 から受理する。
        connected_to(&mut client, &other);
        assert!(
            client
                .receive(workspace_event(&other, 1))
                .unwrap()
                .is_some()
        );

        // close 後は同じ Workspace へ戻っても系列が進んでいる可能性があるため取り直す。
        let (ClientMessage::Request(request), _reply) = client
            .begin(RequestBody::WorkspaceClose(
                client_api::WorkspaceCloseRequest {
                    workspace_id: other.clone(),
                },
            ))
            .unwrap()
        else {
            panic!("request")
        };
        client
            .receive(ServerMessage::Response(ResponseEnvelope {
                request_id: request.request_id,
                body: ResponseBody::WorkspaceClose(client_api::WorkspaceCloseResponse {
                    workspace_id: other.clone(),
                }),
            }))
            .unwrap();
        connected_to(&mut client, &other);
        assert!(
            client
                .receive(workspace_event(&other, 9))
                .unwrap()
                .is_some()
        );
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
        use client_api::{
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
        use client_api::{
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
