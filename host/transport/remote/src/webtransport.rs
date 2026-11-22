//! WebTransport (HTTP/3 over QUIC) のセッション確立。
//!
//! HTTP/3 のハンドシェイク（ALPN `h3`、SETTINGS、CONNECT の QPACK ヘッダー）は
//! `web-transport-quinn` に任せ、Lapis 固有のメッセージ経路だけを持つ。
//! ALPN・QPACK・stream 種別の細部はブラウザーの実装に強く依存し、自作すると
//! ブラウザーの更新ごとに差分を追うことになるため、確立部分は実績のある実装を使う。
//!
//! 確立後はブラウザーが開く最初の bidi stream を WebTransport のメッセージ経路とし、
//! QUIC 経路と同じ長さ付きフレームで `ClientMessage` / `ServerMessage` を運ぶ。

use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use web_transport_quinn::Session;

/// 確立処理で待つ時間の上限。ブラウザーは接続直後に制御ストリームを開く。
pub(crate) const SETUP_TIMEOUT: Duration = Duration::from_secs(10);

/// WebTransport の ALPN。HTTP/3 上でだけ成立する。
pub(crate) const ALPN: &str = web_transport_quinn::ALPN;

/// WebTransport セッションの確立で失敗した理由。
#[derive(Debug)]
pub enum SessionError {
    /// 接続が閉じた、または待ち時間を過ぎた。
    Transport,
    /// HTTP/3 / WebTransport の前提を満たさない。
    Protocol(String),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport => {
                formatter.write_str("connection closed before the session was accepted")
            }
            Self::Protocol(detail) => write!(formatter, "invalid WebTransport session: {detail}"),
        }
    }
}

impl std::error::Error for SessionError {}

/// CONNECT ストリームの受信側。Session と一緒に保持し、drop でセッションを終える。
pub(crate) struct SessionReader {
    /// セッションの寿命を持つ。field の drop 順で `recv` より後に解放される。
    #[allow(dead_code)]
    session: Session,
    recv: web_transport_quinn::RecvStream,
}

/// CONNECT ストリームの送信側。
pub(crate) struct SessionWriter {
    #[allow(dead_code)]
    session: Session,
    send: web_transport_quinn::SendStream,
}

impl AsyncRead for SessionReader {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.recv).poll_read(cx, buf)
    }
}

impl AsyncWrite for SessionWriter {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.send).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.send).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.send).poll_shutdown(cx)
    }
}

/// サーバー側で WebTransport セッションを確立し、メッセージ経路の入出力を返す。
///
/// 応答（200）まで済ませた後、ブラウザーが開く最初の bidi stream を
/// メッセージ経路として受け取る。
pub(crate) async fn accept(
    connection: quinn::Connection,
) -> Result<(SessionReader, SessionWriter), SessionError> {
    let setup = async {
        let request = web_transport_quinn::Request::accept(connection)
            .await
            .map_err(|error| SessionError::Protocol(error.to_string()))?;
        let session = request
            .ok()
            .await
            .map_err(|error| SessionError::Protocol(error.to_string()))?;
        let (send, recv) = session
            .accept_bi()
            .await
            .map_err(|error| SessionError::Protocol(error.to_string()))?;
        Ok::<_, SessionError>((session, send, recv))
    };
    let (session, send, recv) = tokio::time::timeout(SETUP_TIMEOUT, setup)
        .await
        .map_err(|_| SessionError::Transport)??;
    Ok((
        SessionReader {
            session: session.clone(),
            recv,
        },
        SessionWriter { session, send },
    ))
}
