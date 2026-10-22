use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

/// QUICもWebSocketと同じSession処理へ配送する。受信途中のselectキャンセルでも
/// codecが部分フレームを保持し、イベント配信によるメッセージ欠落を防ぐ。
pub(crate) enum RemoteSocket {
    WebSocket(WebSocket),
    Quic {
        connection: quinn::Connection,
        reader: FramedRead<quinn::RecvStream, LengthDelimitedCodec>,
        writer: FramedWrite<quinn::SendStream, LengthDelimitedCodec>,
    },
}

impl RemoteSocket {
    pub(crate) async fn recv(&mut self) -> Option<Result<Message, ()>> {
        match self {
            Self::WebSocket(socket) => socket.recv().await.map(|result| result.map_err(|_| ())),
            Self::Quic { reader, .. } => reader.next().await.map(|result| {
                let bytes = result.map_err(|_| ())?;
                let text = String::from_utf8(bytes.to_vec()).map_err(|_| ())?;
                Ok(Message::Text(text.into()))
            }),
        }
    }

    pub(crate) async fn send(&mut self, message: Message) -> Result<(), ()> {
        match self {
            Self::WebSocket(socket) => socket.send(message).await.map_err(|_| ()),
            Self::Quic {
                connection, writer, ..
            } => match message {
                Message::Text(text) => writer
                    .send(text.as_bytes().to_vec().into())
                    .await
                    .map_err(|_| ()),
                Message::Close(_) => {
                    writer.close().await.map_err(|_| ())?;
                    // エラー応答をACKされる前にcloseすると、QUICは未送信データを破棄する。
                    writer.get_mut().stopped().await.map_err(|_| ())?;
                    connection.close(0u32.into(), b"session closed");
                    Ok(())
                }
                _ => Err(()),
            },
        }
    }
}

impl Drop for RemoteSocket {
    fn drop(&mut self) {
        if let Self::Quic { connection, .. } = self {
            connection.close(0u32.into(), b"session closed");
        }
    }
}
