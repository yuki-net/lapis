use std::{error::Error, fmt, future::Future, pin::Pin};

use lapis_client_api::{EventEnvelope, RequestEnvelope, ResponseEnvelope};

use crate::SessionGrant;

pub type RemoteResponseFuture<'a> = Pin<Box<dyn Future<Output = ResponseEnvelope> + Send + 'a>>;
pub type RemoteSubscriptionFuture<'a> = Pin<
    Box<
        dyn Future<Output = Result<Option<RemoteEventReceiver>, RemoteSubscriptionError>>
            + Send
            + 'a,
    >,
>;

pub struct RemoteEventReceiver {
    receiver: tokio::sync::mpsc::Receiver<EventEnvelope>,
}

impl RemoteEventReceiver {
    pub(crate) fn new(receiver: tokio::sync::mpsc::Receiver<EventEnvelope>) -> Self {
        Self { receiver }
    }

    pub async fn recv(&mut self) -> Option<EventEnvelope> {
        self.receiver.recv().await
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemoteSubscriptionError;

impl fmt::Display for RemoteSubscriptionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("remote event subscription is unavailable")
    }
}

impl Error for RemoteSubscriptionError {}

/// Transportとbackend正規状態を分離するrequest処理port。
///
/// 実装側はDocumentやTerminalなどのresource所有権をSession/Workspaceへ結び付けて検証する。
pub trait RemoteRequestHandler: Send + Sync + 'static {
    fn handle(&self, session: SessionGrant, request: RequestEnvelope) -> RemoteResponseFuture<'_>;

    fn subscribe(&self, _session: SessionGrant) -> RemoteSubscriptionFuture<'_> {
        Box::pin(async { Ok(None) })
    }

    fn disconnect(&self, _session: &SessionGrant) {}
}
