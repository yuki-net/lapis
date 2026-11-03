//! DesktopとWebで共通に使う非同期要求の順序と接続寿命。

use std::collections::HashMap;

use futures_channel::oneshot;
use lapis_client_api::{
    ClientMessage, EventEnvelope, EventSequence, RequestBody, RequestEnvelope, RequestId,
    ResponseBody, ServerMessage,
};

const MAX_PENDING: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientError {
    Disconnected,
    TooManyRequests,
    CounterOverflow,
    Protocol,
    EventGap,
}

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
}
