use std::{future::Future, pin::Pin};

use lapis_client_api::{RequestEnvelope, ResponseEnvelope};

use crate::SessionGrant;

pub type RemoteResponseFuture<'a> = Pin<Box<dyn Future<Output = ResponseEnvelope> + Send + 'a>>;

/// Transportとbackend正規状態を分離するrequest処理port。
///
/// 実装側はDocumentやTerminalなどのresource所有権をSession/Workspaceへ結び付けて検証する。
pub trait RemoteRequestHandler: Send + Sync + 'static {
    fn handle(&self, session: SessionGrant, request: RequestEnvelope) -> RemoteResponseFuture<'_>;
}
