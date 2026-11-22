use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

use crate::webtransport::{ConnectReader, ConnectWriter};

type Frames = LengthDelimitedCodec;

fn text(bytes: bytes::BytesMut) -> Result<Message, ()> {
    let text = String::from_utf8(bytes.to_vec()).map_err(|_| ())?;
    Ok(Message::Text(text.into()))
}

/// Clientの接続経路。Session処理はすべて同じで、読み書きの primitive だけが異なる。
///
/// 受信中に event 配送へ切り替えても、codec が部分フレームを保持する。
pub(crate) enum RemoteSocket {
    WebSocket(WebSocket),
    Quic {
        connection: quinn::Connection,
        reader: FramedRead<quinn::RecvStream, Frames>,
        writer: FramedWrite<quinn::SendStream, Frames>,
    },
    /// WebTransport の CONNECT ストリーム。同一 QUIC 接続で複数セッションを
    /// 併存できるため、終了はこのストリームの FIN だけで、接続は閉じない。
    WebTransport {
        reader: FramedRead<ConnectReader, Frames>,
        writer: FramedWrite<ConnectWriter, Frames>,
    },
}

impl RemoteSocket {
    pub(crate) async fn recv(&mut self) -> Option<Result<Message, ()>> {
        match self {
            Self::WebSocket(socket) => socket.recv().await.map(|result| result.map_err(|_| ())),
            Self::Quic { reader, .. } => reader
                .next()
                .await
                .map(|result| result.map_err(|_| ()).and_then(text)),
            Self::WebTransport { reader, .. } => reader
                .next()
                .await
                .map(|result| result.map_err(|_| ()).and_then(text)),
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
            Self::WebTransport { writer, .. } => match message {
                Message::Text(text) => writer
                    .send(text.as_bytes().to_vec().into())
                    .await
                    .map_err(|_| ()),
                Message::Close(_) => {
                    writer.close().await.map_err(|_| ())?;
                    // FIN を送るだけでは未送信データが破棄され得るため、peer が stream を
                    // 読み切って stopped() を返すまで待つ。接続自体は閉じない。
                    let _ = writer.get_mut().0.stopped().await;
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
