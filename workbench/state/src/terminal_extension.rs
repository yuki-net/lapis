//! Terminal 拡張の版付き DTO（v1）と、クライアント側の出力カーソル・描画器。

pub use terminal_session_api::v1;

#[cfg(any(target_arch = "wasm32", test))]
pub(crate) mod web_projection;

/// 各 Terminal の生出力を一度だけ、順番に画面へ適用するための位置。
pub struct OutputCursor {
    id: String,
    offset: u64,
    generation: u64,
    pending: Option<(u64, u32, Option<u64>)>,
}

/// 接続変更・要求取消後の古い応答を判別する。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadTicket(u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadError {
    Pending,
    Stale,
    InvalidRequest,
    InvalidResponse,
    /// 画面を再構築する必要があり、部分的な出力を適用してはいけない。
    OutputLost {
        expected: u64,
        available_from: u64,
    },
}

/// Desktop のネイティブ描画器。通信と process は拡張側が所有する。
#[cfg(not(target_arch = "wasm32"))]
pub struct NativeRenderer {
    cursor: OutputCursor,
    engine: terminal_renderer::TerminalEngine,
}

#[cfg(not(target_arch = "wasm32"))]
pub struct NativeOutput {
    pub delta: Option<terminal::TerminalDelta>,
    pub exit: Option<v1::Exit>,
    pub read_error: Option<String>,
    pub write_error: Option<String>,
    pub owner: Option<String>,
}

/// Web でも利用できる描画器と、順序を検証する出力位置。
pub struct PortableRenderer {
    cursor: OutputCursor,
    engine: terminal_portable_renderer::Renderer,
}

pub struct PortableOutput {
    pub screen: terminal::TerminalScreen,
    pub reply: Vec<u8>,
    pub exit: Option<v1::Exit>,
    pub read_error: Option<String>,
    pub write_error: Option<String>,
    pub owner: Option<String>,
}

impl PortableRenderer {
    pub fn new(id: String, size: terminal::TerminalSize) -> Result<Self, String> {
        Self::with_pty(id, size, v1::PtyFlavor::Posix)
    }

    pub fn with_pty(
        id: String,
        size: terminal::TerminalSize,
        pty: v1::PtyFlavor,
    ) -> Result<Self, String> {
        Ok(Self {
            cursor: OutputCursor::new(id),
            engine: terminal_portable_renderer::Renderer::with_conpty(
                size,
                pty == v1::PtyFlavor::Conpty,
            )?,
        })
    }

    pub fn begin(
        &mut self,
        max_bytes: u32,
        timeout_ms: u32,
    ) -> Result<(ReadTicket, v1::ReadRequest), ReadError> {
        self.cursor.begin(max_bytes, timeout_ms)
    }

    pub fn cancel(&mut self) {
        self.cursor.cancel();
    }

    pub fn accept(
        &mut self,
        ticket: ReadTicket,
        output: v1::ReadResponse,
    ) -> Result<PortableOutput, ReadError> {
        let output = self.cursor.accept(ticket, output)?;
        let reply = self.engine.process(&output.bytes);
        let screen = self.engine.snapshot();
        self.cursor.commit(ticket)?;
        Ok(PortableOutput {
            screen,
            reply,
            exit: output.exit,
            read_error: output.read_error,
            write_error: output.write_error,
            owner: output.owner,
        })
    }

    pub fn resize(
        &mut self,
        size: terminal::TerminalSize,
    ) -> Result<terminal::TerminalScreen, String> {
        self.engine.resize(size)?;
        Ok(self.engine.snapshot())
    }

    pub fn input(&self, input: &terminal::TerminalInput) -> Result<Vec<u8>, String> {
        self.engine.input(input)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl NativeRenderer {
    /// writer は非同期に Host へ送る入力キューとする。
    pub fn new(
        id: String,
        size: terminal::TerminalSize,
        writer: Box<dyn std::io::Write + Send>,
        pty_flavor: v1::PtyFlavor,
    ) -> Self {
        Self {
            cursor: OutputCursor::new(id),
            engine: terminal_renderer::TerminalEngine::with_conpty_quirks(
                size,
                writer,
                pty_flavor == v1::PtyFlavor::Conpty,
            ),
        }
    }

    pub fn begin(
        &mut self,
        max_bytes: u32,
        timeout_ms: u32,
    ) -> Result<(ReadTicket, v1::ReadRequest), ReadError> {
        self.cursor.begin(max_bytes, timeout_ms)
    }

    pub fn cancel(&mut self) {
        self.cursor.cancel();
    }

    pub fn accept(
        &mut self,
        ticket: ReadTicket,
        output: v1::ReadResponse,
    ) -> Result<NativeOutput, ReadError> {
        let output = self.cursor.accept(ticket, output)?;
        let delta = if output.bytes.is_empty() {
            None
        } else {
            Some(self.engine.process(&output.bytes))
        };
        self.cursor.commit(ticket)?;
        Ok(NativeOutput {
            delta,
            exit: output.exit,
            read_error: output.read_error,
            write_error: output.write_error,
            owner: output.owner,
        })
    }

    pub fn snapshot(&self) -> &terminal::TerminalScreen {
        self.engine.snapshot()
    }

    pub fn resize(
        &mut self,
        size: terminal::TerminalSize,
    ) -> (terminal::TerminalScreen, terminal::TerminalDelta) {
        self.engine.resize(size)
    }

    pub fn input(
        &mut self,
        input: &terminal::TerminalInput,
    ) -> Result<(), terminal::TerminalError> {
        self.engine.input(input)
    }

    pub fn take_write_failures(&self) -> impl Iterator<Item = String> + '_ {
        self.engine.take_failures()
    }
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => formatter.write_str("Terminal の出力取得は既に実行中です"),
            Self::Stale => formatter.write_str("古い Terminal の出力を破棄しました"),
            Self::InvalidRequest => formatter.write_str("Terminal の出力取得条件が不正です"),
            Self::InvalidResponse => formatter.write_str("Terminal の出力応答が不正です"),
            Self::OutputLost {
                expected,
                available_from,
            } => write!(
                formatter,
                "Terminal の出力が欠落しました: {expected}..{available_from}"
            ),
        }
    }
}

impl OutputCursor {
    pub fn new(id: String) -> Self {
        Self {
            id,
            offset: 0,
            generation: 0,
            pending: None,
        }
    }

    pub fn offset(&self) -> u64 {
        self.offset
    }

    pub fn begin(
        &mut self,
        max_bytes: u32,
        timeout_ms: u32,
    ) -> Result<(ReadTicket, v1::ReadRequest), ReadError> {
        if self.pending.is_some() {
            return Err(ReadError::Pending);
        }
        if max_bytes == 0 || max_bytes > v1::MAX_READ_BYTES || timeout_ms > v1::MAX_READ_WAIT_MS {
            return Err(ReadError::InvalidRequest);
        }
        self.generation = self.generation.wrapping_add(1);
        self.pending = Some((self.generation, max_bytes, None));
        Ok((
            ReadTicket(self.generation),
            v1::ReadRequest {
                id: self.id.clone(),
                offset: self.offset,
                max_bytes,
                timeout_ms,
            },
        ))
    }

    /// 未完了要求を無効にする。表示済みの出力位置は保持する。
    pub fn cancel(&mut self) {
        self.pending = None;
    }

    /// 応答を検証する。画面への反映後に commit が必要。
    pub fn accept(
        &mut self,
        ticket: ReadTicket,
        output: v1::ReadResponse,
    ) -> Result<v1::ReadResponse, ReadError> {
        let Some((generation, max_bytes, ready)) = self.pending else {
            return Err(ReadError::Stale);
        };
        if generation != ticket.0 || ready.is_some() {
            return Err(ReadError::Stale);
        }
        self.pending = None;
        if let Some(available_from) = output.lost_before {
            if available_from <= self.offset
                || output.bytes.len() > max_bytes as usize
                || available_from.checked_add(output.bytes.len() as u64) != Some(output.next)
            {
                return Err(ReadError::InvalidResponse);
            }
            return Err(ReadError::OutputLost {
                expected: self.offset,
                available_from,
            });
        }
        let next = self
            .offset
            .checked_add(output.bytes.len() as u64)
            .ok_or(ReadError::InvalidResponse)?;
        if output.next != next || output.bytes.len() > max_bytes as usize {
            return Err(ReadError::InvalidResponse);
        }
        self.pending = Some((generation, max_bytes, Some(next)));
        Ok(output)
    }

    /// 検証済みの出力を描画状態へ反映できたときだけ位置を確定する。
    pub fn commit(&mut self, ticket: ReadTicket) -> Result<(), ReadError> {
        match self.pending {
            Some((generation, _, Some(next))) if generation == ticket.0 => {
                self.offset = next;
                self.pending = None;
                Ok(())
            }
            _ => Err(ReadError::Stale),
        }
    }

    pub fn ready(&self, ticket: ReadTicket) -> bool {
        matches!(self.pending, Some((generation, _, Some(_))) if generation == ticket.0)
    }

    pub fn discard(&mut self, ticket: ReadTicket) -> Result<(), ReadError> {
        if !matches!(self.pending, Some((generation, _, _)) if generation == ticket.0) {
            return Err(ReadError::Stale);
        }
        self.pending = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(bytes: &[u8], next: u64, lost_before: Option<u64>) -> v1::ReadResponse {
        v1::ReadResponse {
            bytes: bytes.to_vec(),
            next,
            lost_before,
            exit: None,
            read_error: None,
            write_error: None,
            owner: None,
        }
    }

    #[test]
    fn output_cursor_rejects_stale_lost_and_malformed_output() {
        let mut cursor = OutputCursor::new("pty-1".into());
        let (first, request) = cursor.begin(32, 100).unwrap();
        assert_eq!((request.id.as_str(), request.offset), ("pty-1", 0));
        assert!(matches!(cursor.begin(32, 100), Err(ReadError::Pending)));
        cursor.cancel();
        let (second, _) = cursor.begin(32, 100).unwrap();
        assert_eq!(
            cursor.accept(first, output(b"old", 3, None)),
            Err(ReadError::Stale)
        );
        assert_eq!(
            cursor
                .accept(second, output(b"abc", 3, None))
                .unwrap()
                .bytes,
            b"abc"
        );
        assert_eq!(cursor.offset(), 0);
        assert!(matches!(cursor.begin(32, 100), Err(ReadError::Pending)));
        assert_eq!(cursor.commit(first), Err(ReadError::Stale));
        cursor.commit(second).unwrap();
        assert_eq!(cursor.offset(), 3);
        let (third, request) = cursor.begin(32, 100).unwrap();
        assert_eq!(request.offset, 3);
        assert_eq!(
            cursor.accept(third, output(b"x", 9, Some(8))),
            Err(ReadError::OutputLost {
                expected: 3,
                available_from: 8
            })
        );
        assert_eq!(cursor.offset(), 3);
        let (fourth, _) = cursor.begin(32, 100).unwrap();
        assert_eq!(
            cursor.accept(fourth, output(b"x", 10, Some(8))),
            Err(ReadError::InvalidResponse)
        );
        assert_eq!(cursor.offset(), 3);
        let (fifth, _) = cursor.begin(32, 100).unwrap();
        assert_eq!(
            cursor.accept(fifth, output(b"de", 5, None)).unwrap().bytes,
            b"de"
        );
        cursor.commit(fifth).unwrap();
        assert_eq!(cursor.offset(), 5);
        assert!(matches!(cursor.begin(0, 0), Err(ReadError::InvalidRequest)));
        assert!(matches!(
            cursor.begin(1, v1::MAX_READ_WAIT_MS + 1),
            Err(ReadError::InvalidRequest)
        ));
        let (sixth, _) = cursor.begin(1, 0).unwrap();
        assert_eq!(
            cursor.accept(sixth, output(b"fg", 7, None)),
            Err(ReadError::InvalidResponse)
        );
        assert_eq!(cursor.offset(), 5);
        let (seventh, _) = cursor.begin(2, 0).unwrap();
        cursor.accept(seventh, output(b"fg", 7, None)).unwrap();
        cursor.cancel();
        assert_eq!(cursor.commit(seventh), Err(ReadError::Stale));
        assert_eq!(cursor.offset(), 5);
        assert_eq!(cursor.begin(2, 0).unwrap().1.offset, 5);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_renderer_applies_only_contiguous_extension_output() {
        let mut renderer = NativeRenderer::new(
            "pty-1".into(),
            terminal::TerminalSize::new(20, 3),
            Box::new(std::io::sink()),
            v1::PtyFlavor::Posix,
        );
        let (ticket, request) = renderer.begin(32, 0).unwrap();
        assert_eq!(request.offset, 0);
        let applied = renderer.accept(ticket, output(b"hello", 5, None)).unwrap();
        assert!(applied.delta.is_some());
        assert!(renderer.snapshot().plain_text().contains("hello"));
        let before = renderer.snapshot().clone();
        let (ticket, request) = renderer.begin(32, 0).unwrap();
        assert_eq!(request.offset, 5);
        assert!(matches!(
            renderer.accept(ticket, output(b"x", 11, Some(10))),
            Err(ReadError::OutputLost {
                expected: 5,
                available_from: 10
            })
        ));
        assert_eq!(renderer.snapshot(), &before);
        let (ticket, request) = renderer.begin(32, 0).unwrap();
        assert_eq!(request.offset, 5);
        renderer
            .accept(ticket, output(b" world", 11, None))
            .unwrap();
        assert!(renderer.snapshot().plain_text().contains("hello world"));
    }

    #[test]
    fn portable_renderer_checks_order_and_replies_to_cursor_queries() {
        let mut renderer =
            PortableRenderer::new("pty-1".into(), terminal::TerminalSize::new(20, 3)).unwrap();
        let (ticket, _) = renderer.begin(32, 0).unwrap();
        let applied = renderer
            .accept(ticket, output(b"abc\x1b[6n", 7, None))
            .unwrap();
        assert_eq!(applied.reply, b"\x1b[1;4R");
        assert!(applied.screen.plain_text().contains("abc"));
        let (ticket, request) = renderer.begin(32, 0).unwrap();
        assert_eq!(request.offset, 7);
        assert!(matches!(
            renderer.accept(ticket, output(b"x", 21, Some(20))),
            Err(ReadError::OutputLost { .. })
        ));
        let (ticket, request) = renderer.begin(32, 0).unwrap();
        assert_eq!(request.offset, 7);
        assert!(
            renderer
                .accept(ticket, output(b"d", 8, None))
                .unwrap()
                .screen
                .plain_text()
                .contains("abcd")
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn portable_input_matches_native_for_common_keys() {
        use std::{
            io::{self, Write},
            sync::mpsc,
            time::Duration,
        };
        use terminal::{TerminalInput, TerminalKey, TerminalKeyEvent, TerminalModifiers};
        struct Capture(mpsc::Sender<Vec<u8>>);
        impl Write for Capture {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0.send(bytes.to_vec()).map_err(io::Error::other)?;
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let (sender, receiver) = mpsc::channel();
        let size = terminal::TerminalSize::new(80, 24);
        let conpty = cfg!(windows);
        let mut native = terminal_renderer::TerminalEngine::with_conpty_quirks(
            size,
            Box::new(Capture(sender)),
            conpty,
        );
        let portable = terminal_portable_renderer::Renderer::with_conpty(size, conpty).unwrap();
        let key = |key, modifiers| TerminalInput::Key(TerminalKeyEvent { key, modifiers });
        let control = TerminalModifiers {
            control: true,
            ..Default::default()
        };
        let alt = TerminalModifiers {
            alt: true,
            ..Default::default()
        };
        let shift = TerminalModifiers {
            shift: true,
            ..Default::default()
        };
        let inputs = [
            key(TerminalKey::Character("c".into()), control),
            key(TerminalKey::Character(" ".into()), control),
            key(TerminalKey::Backspace, alt),
            key(TerminalKey::Tab, shift),
            key(TerminalKey::Tab, control),
            key(TerminalKey::Tab, alt),
            key(TerminalKey::ArrowUp, Default::default()),
            key(TerminalKey::ArrowUp, alt),
            key(TerminalKey::ArrowUp, control),
            key(TerminalKey::ArrowDown, shift),
            key(TerminalKey::Home, Default::default()),
            key(TerminalKey::Home, alt),
            key(TerminalKey::Delete, Default::default()),
            key(TerminalKey::Insert, shift),
            key(TerminalKey::Function(1), Default::default()),
            key(TerminalKey::Function(2), alt),
            key(TerminalKey::Function(5), control),
            TerminalInput::Paste("one\ntwo".into()),
        ];
        for input in inputs {
            native.input(&input).unwrap();
            let expected = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(portable.input(&input).unwrap(), expected, "{input:?}");
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_renderer_answers_remote_pty_cursor_queries() {
        struct Capture(std::sync::mpsc::Sender<Vec<u8>>);
        impl std::io::Write for Capture {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.send(bytes.to_vec()).map_err(std::io::Error::other)?;
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut renderer = NativeRenderer::new(
            "pty-1".into(),
            terminal::TerminalSize::new(20, 3),
            Box::new(Capture(sender)),
            v1::PtyFlavor::Conpty,
        );
        let (ticket, _) = renderer.begin(32, 0).unwrap();
        renderer
            .accept(ticket, output(b"\x1b[6n", 4, None))
            .unwrap();
        assert_eq!(
            receiver
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap(),
            b"\x1b[1;1R"
        );
    }
}
