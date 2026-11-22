//! WebTransport (HTTP/3 over QUIC) のセッション確立に必要な最小限の処理。
//!
//! 用途は Lapis の Client と Host が同じ WebTransport セッションを張ることだけであり、
//! 一般の HTTP/3（request/response、GOAWAY、QPACK の動的テーブルなど）は扱わない。
//! 各接続で Client が開く制御ストリーム（SETTINGS）を読み、CONNECT ストリームの
//! request を検証して 200 を返したら、CONNECT ストリームをそのまま
//! `LengthDelimitedCodec` で扱うメッセージ経路へ渡す。
//!
//! 接続の向きで SETTINGS を書く側と読む側が入れ替わるため、確立処理は
//! `SettingsWriter` / `SettingsReader` の 2 段に分ける。

use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use web_transport_proto::{ConnectRequest, ConnectResponse, Settings};

/// 確立処理で待つ時間の上限。ブラウザーは接続直後に制御ストリームを開く。
pub(crate) const SETUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// WebTransport セッションの確立で失敗した理由。
#[derive(Debug)]
pub enum SessionError {
    /// 接続が閉じた、または待ち時間を過ぎた。
    Transport,
    /// HTTP/3 の前提を満たさない stream だった。
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

impl From<BoxError> for SessionError {
    fn from(error: BoxError) -> Self {
        Self::Protocol(error.to_string())
    }
}

/// CONNECT ストリームの受信側。
pub struct ConnectReader(pub(crate) quinn::RecvStream);

/// CONNECT ストリームの送信側。
pub struct ConnectWriter(pub(crate) quinn::SendStream);

/// 確立済みの WebTransport セッションの入出力。
#[derive(Debug)]
pub struct ConnectStream {
    pub(crate) send: quinn::SendStream,
    pub(crate) recv: quinn::RecvStream,
}

impl ConnectStream {
    /// 読み取りと書き込みを独立して扱えるよう、1 本の CONNECT ストリームを分ける。
    pub fn split(self) -> (ConnectReader, ConnectWriter) {
        (ConnectReader(self.recv), ConnectWriter(self.send))
    }
}

impl AsyncRead for ConnectReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0)
            .poll_read(cx, buf)
            .map_err(|error| io::Error::new(io::ErrorKind::BrokenPipe, error))
    }
}

impl AsyncWrite for ConnectWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0)
            .poll_write(cx, buf)
            .map_err(|error| io::Error::new(io::ErrorKind::BrokenPipe, error))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

/// 制御ストリームを開いて SETTINGS を書く。ブラウザーに WebTransport の対応を伝える。
pub(crate) async fn write_settings(connection: &quinn::Connection) -> Result<(), SessionError> {
    let mut stream = connection
        .open_uni()
        .await
        .map_err(|_| SessionError::Transport)?;
    settings()
        .write(&mut stream)
        .await
        .map_err(|error| SessionError::Protocol(error.to_string()))?;
    Ok(())
}

/// Host が広告する SETTINGS。WebTransport を有効にし、追加セッションは受け付けない。
fn settings() -> Settings {
    let mut settings = Settings::default();
    settings.enable_webtransport(1);
    settings
}

/// 制御ストリームから SETTINGS を読み、WebTransport が有効かを確認する。
pub(crate) async fn read_settings(mut stream: quinn::RecvStream) -> Result<(), SessionError> {
    let settings = Settings::read(&mut stream)
        .await
        .map_err(|error| SessionError::Protocol(error.to_string()))?;
    if settings.supports_webtransport() == 0 {
        return Err(SessionError::Protocol(
            "peer does not enable WebTransport".to_owned(),
        ));
    }
    Ok(())
}

/// 双方向 CONNECT ストリームの request を読み、WebTransport の要求として検証する。
pub(crate) async fn read_connect(
    stream: &mut quinn::RecvStream,
) -> Result<ConnectRequest, SessionError> {
    ConnectRequest::read(stream)
        .await
        .map_err(|error| SessionError::Protocol(error.to_string()))
}

/// CONNECT ストリームへ 200 を書く。
pub(crate) async fn write_connect_ok(stream: &mut quinn::SendStream) -> Result<(), SessionError> {
    ConnectResponse::OK
        .write(stream)
        .await
        .map_err(|error| SessionError::Protocol(error.to_string()))
}

/// クライアント側の WebTransport セッション。ブラウザーの `new WebTransport()` と
/// 同じ順序（制御ストリーム → CONNECT ストリーム）で確立する。
///
/// ブラウザーの実装を差し替えずに Host 側の受け付けを検証するためと、
/// 将来 Desktop からの遠隔接続で HTTP/3 を使う場合の土台を兼ねる。
pub async fn connect_session(
    connection: &quinn::Connection,
    authority: &str,
) -> Result<ConnectStream, SessionError> {
    write_settings(connection).await?;
    let request = ConnectRequest::new(
        format!("https://{authority}/lapis")
            .parse::<url::Url>()
            .map_err(|_| SessionError::Protocol("invalid authority".to_owned()))?,
    );
    let (mut send, mut recv) = connection
        .open_bi()
        .await
        .map_err(|_| SessionError::Transport)?;
    request
        .write(&mut send)
        .await
        .map_err(|error| SessionError::Protocol(error.to_string()))?;
    let response = ConnectResponse::read(&mut recv)
        .await
        .map_err(|error| SessionError::Protocol(error.to_string()))?;
    if !response.status.is_success() {
        return Err(SessionError::Protocol(format!(
            "server rejected the session: {}",
            response.status
        )));
    }
    Ok(ConnectStream { send, recv })
}
