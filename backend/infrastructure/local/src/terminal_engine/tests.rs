use super::*;

#[test]
#[ignore]
fn engine_keeps_split_utf8_and_combining_cells_in_the_model() {
    // wezterm-termの既知制限:
    // base characterとcombining markが別advance_bytes chunkに分割された場合、
    // combining markが失われる。P0では許容し、Lapis独自補正は行わない。
    // https://github.com/wezterm/wezterm/issues/
    // 将来revision更新時に修正確認できる状態にする。
    let (mut engine, _replies) = engine();
    let text = "你e".as_bytes();
    engine.process(&text[..1]);
    engine.process(&text[1..]);
    engine.process("\u{301}".as_bytes());
    let screen = engine.screen();

    assert_eq!(screen.rows[0].cells[0].text, "你");
    assert_eq!(screen.rows[0].cells[0].width, 2);
    assert_eq!(screen.rows[0].cells[1].width, 0);
    assert_eq!(screen.rows[0].cells[2].text, "e\u{301}");
}

#[test]
fn engine_exports_attributes_colors_modes_and_cursor() {
    let (mut engine, _replies) = engine();
    engine.process(b"\x1b[1;3;4;31;44mA\x1b[2;5H");
    let screen = engine.screen();

    let cell = &screen.rows[0].cells[0];
    assert_eq!(cell.foreground, TerminalColor::Indexed(1));
    assert_eq!(cell.background, TerminalColor::Indexed(4));
    assert!(cell.bold && cell.italic && cell.underline);
    assert_eq!(screen.cursor.row, 1);
    assert_eq!(screen.cursor.column, 4);
    assert!(screen.cursor.visible);
}

#[test]
fn engine_tracks_alternate_screen_and_answers_terminal_queries() {
    let (mut engine, replies) = engine();
    engine.process(b"\x1b[?1049hALT");
    assert!(engine.screen().modes.alternate_screen);

    engine.process(b"\x1b[6n");
    assert_eq!(
        replies.recv_timeout(Duration::from_secs(5)).unwrap(),
        b"\x1b[1;4R"
    );

    engine.process(b"\x1b[?1049l");
    assert!(!engine.screen().modes.alternate_screen);
}

use lapis_terminal::TerminalKeyEvent;
use std::time::Duration;

struct Capture(mpsc::Sender<Vec<u8>>);
impl Write for Capture {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.0.send(data.to_vec()).map_err(io::Error::other)?;
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn engine() -> (TerminalEngine, mpsc::Receiver<Vec<u8>>) {
    let (sender, receiver) = mpsc::channel();
    (
        TerminalEngine::new(TerminalSize::new(40, 4), Box::new(Capture(sender))),
        receiver,
    )
}

fn key(key: TerminalKey, modifiers: TerminalModifiers) -> TerminalInput {
    TerminalInput::Key(TerminalKeyEvent { key, modifiers })
}

#[test]
fn official_keyboard_encoding_covers_semantic_keys_and_modifiers() {
    let cases = [
        (
            TerminalKey::Character("c".into()),
            TerminalModifiers {
                control: true,
                ..Default::default()
            },
            b"\x03".as_slice(),
        ),
        (
            TerminalKey::Character(" ".into()),
            TerminalModifiers {
                control: true,
                ..Default::default()
            },
            b"\x00",
        ),
        (
            TerminalKey::Backspace,
            TerminalModifiers {
                alt: true,
                ..Default::default()
            },
            b"\x1b\x7f",
        ),
        (
            TerminalKey::Tab,
            TerminalModifiers {
                shift: true,
                ..Default::default()
            },
            b"\x1b[Z",
        ),
        (
            TerminalKey::ArrowUp,
            TerminalModifiers::default(),
            b"\x1b[A",
        ),
        (
            TerminalKey::ArrowUp,
            TerminalModifiers {
                alt: true,
                ..Default::default()
            },
            b"\x1b[1;3A",
        ),
        (
            TerminalKey::ArrowUp,
            TerminalModifiers {
                control: true,
                ..Default::default()
            },
            b"\x1b[1;5A",
        ),
        (TerminalKey::Home, TerminalModifiers::default(), b"\x1b[H"),
        (TerminalKey::End, TerminalModifiers::default(), b"\x1b[F"),
        (
            TerminalKey::Insert,
            TerminalModifiers::default(),
            b"\x1b[2~",
        ),
        (
            TerminalKey::Delete,
            TerminalModifiers::default(),
            b"\x1b[3~",
        ),
        (
            TerminalKey::PageUp,
            TerminalModifiers::default(),
            b"\x1b[5~",
        ),
        (
            TerminalKey::PageDown,
            TerminalModifiers::default(),
            b"\x1b[6~",
        ),
        (TerminalKey::Enter, TerminalModifiers::default(), b"\r"),
        (
            TerminalKey::Backspace,
            TerminalModifiers::default(),
            b"\x7f",
        ),
        (TerminalKey::Tab, TerminalModifiers::default(), b"\t"),
        (
            TerminalKey::Function(5),
            TerminalModifiers {
                control: true,
                ..Default::default()
            },
            b"\x1b[15;5~",
        ),
    ];
    for (code, modifiers, expected) in cases {
        let (mut engine, writes) = engine();
        engine.input(&key(code.clone(), modifiers)).unwrap();
        assert_eq!(
            writes.recv_timeout(Duration::from_secs(5)).unwrap(),
            expected,
            "{code:?} {modifiers:?}"
        );
    }
}

#[test]
fn official_encoder_handles_function_keys_and_application_cursor_home_end() {
    let expected: [&[u8]; 12] = [
        b"\x1bOP",
        b"\x1bOQ",
        b"\x1bOR",
        b"\x1bOS",
        b"\x1b[15~",
        b"\x1b[17~",
        b"\x1b[18~",
        b"\x1b[19~",
        b"\x1b[20~",
        b"\x1b[21~",
        b"\x1b[23~",
        b"\x1b[24~",
    ];
    let (mut engine, writes) = engine();
    for (index, expected) in expected.iter().enumerate() {
        engine
            .input(&key(
                TerminalKey::Function(index as u8 + 1),
                Default::default(),
            ))
            .unwrap();
        assert_eq!(
            writes.recv_timeout(Duration::from_secs(5)).unwrap(),
            *expected
        );
    }
    engine.process(b"\x1b[?1h");
    for (code, expected) in [
        (TerminalKey::Home, b"\x1bOH"),
        (TerminalKey::End, b"\x1bOF"),
    ] {
        engine.input(&key(code, Default::default())).unwrap();
        assert_eq!(
            writes.recv_timeout(Duration::from_secs(5)).unwrap(),
            expected
        );
    }
}

#[test]
fn official_encoder_handles_kitty_and_paste_and_committed_ime_text() {
    let (mut engine, writes) = engine();
    // Kitty keyboard modeを有効にする。
    // Ctrl+文字は制御文字としてエンコードされる（CSI-uにはならない）。
    engine.process(b"\x1b[>u");
    engine
        .input(&key(
            TerminalKey::Character("c".into()),
            TerminalModifiers {
                control: true,
                ..Default::default()
            },
        ))
        .unwrap();
    assert_eq!(
        writes.recv_timeout(Duration::from_secs(5)).unwrap(),
        b"\x03"
    );
    // bracketed paste
    engine.process(b"\x1b[?2004h");
    engine
        .input(&TerminalInput::Paste("paste\x1b[201~".into()))
        .unwrap();
    assert_eq!(
        writes.recv_timeout(Duration::from_secs(5)).unwrap(),
        b"\x1b[200~paste\x1b[201~"
    );
    // IME確定テキストはraw byteとして送出される。
    let text = "日本語";
    engine.input(&TerminalInput::Text(text.into())).unwrap();
    let mut actual = Vec::new();
    while actual.len() < text.len() {
        actual.extend(writes.recv_timeout(Duration::from_secs(5)).unwrap());
    }
    assert_eq!(actual, text.as_bytes());
}

struct FailWriter {
    flush: bool,
    attempted: mpsc::Sender<()>,
}
impl Write for FailWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if !self.flush {
            let _ = self.attempted.send(());
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "reply write failure",
            ));
        }
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        let _ = self.attempted.send(());
        Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "reply flush failure",
        ))
    }
}

#[test]
fn reply_write_and_flush_failures_do_not_lose_delta_or_revision() {
    for flush in [false, true] {
        let (attempted, received) = mpsc::channel();
        let mut engine = TerminalEngine::new(
            TerminalSize::new(40, 4),
            Box::new(FailWriter { flush, attempted }),
        );
        let first = engine.process(b"before\x1b[6n");
        let mut screen = TerminalScreen::empty(first.size);
        screen.apply_delta(&first).unwrap();
        received.recv_timeout(Duration::from_secs(5)).unwrap();
        // writerの完了通知とfailure通知を同期して確認する。
        let failure = engine
            .failures
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert!(failure.contains("reply"));
        let next = engine.process(b"after");
        assert_eq!(next.revision, first.revision + 1);
        screen.apply_delta(&next).unwrap();
        assert!(screen.plain_text().contains("beforeafter"));
        // scrollbackは空なので、比較時に除外する
        let mut expected_screen = engine.screen();
        expected_screen.scrollback.clear();
        screen.scrollback.clear();
        assert_eq!(screen, expected_screen);
    }
}
