//! Web でも使用できる Terminal の出力描画器。

use terminal::{
    MAX_SCROLLBACK_LINES, TerminalCell, TerminalColor, TerminalCursor, TerminalInput, TerminalKey,
    TerminalModes, TerminalModifiers, TerminalRow, TerminalScreen, TerminalSize,
};

#[derive(Default)]
struct Replies(Vec<u8>);

impl vt100::Callbacks for Replies {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        first: Option<u8>,
        second: Option<u8>,
        params: &[&[u16]],
        command: char,
    ) {
        if command != 'n' || second.is_some() || params.len() != 1 {
            return;
        }
        match (first, params[0]) {
            (None, [5]) => self.0.extend_from_slice(b"\x1b[0n"),
            (None | Some(b'?'), [6]) => {
                let (row, column) = screen.cursor_position();
                let prefix = if first.is_some() { "?" } else { "" };
                self.0.extend_from_slice(
                    format!("\x1b[{prefix}{};{}R", row + 1, column + 1).as_bytes(),
                );
            }
            _ => {}
        }
    }
}

/// raw PTY bytes を画面に反映し、端末からの問い合わせへの応答を返す。
pub struct Renderer {
    parser: vt100::Parser<Replies>,
    size: TerminalSize,
    conpty: bool,
}

impl Renderer {
    pub fn new(size: TerminalSize) -> Result<Self, String> {
        Self::with_conpty(size, false)
    }

    pub fn with_conpty(size: TerminalSize, conpty: bool) -> Result<Self, String> {
        validate(size)?;
        Ok(Self {
            parser: vt100::Parser::new_with_callbacks(
                size.rows,
                size.columns,
                MAX_SCROLLBACK_LINES,
                Replies::default(),
            ),
            size,
            conpty,
        })
    }

    pub fn process(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.parser.process(bytes);
        std::mem::take(&mut self.parser.callbacks_mut().0)
    }

    pub fn resize(&mut self, size: TerminalSize) -> Result<(), String> {
        validate(size)?;
        self.parser.screen_mut().set_size(size.rows, size.columns);
        self.size = size;
        Ok(())
    }

    /// 端末の入力モードを使って UI の入力を PTY bytes に変換する。
    pub fn input(&self, input: &TerminalInput) -> Result<Vec<u8>, String> {
        match input {
            TerminalInput::Text(text) => Ok(text.as_bytes().to_vec()),
            TerminalInput::Paste(text) => {
                let clean = text.replace("\x1b[200~", "").replace("\x1b[201~", "");
                if self.parser.screen().bracketed_paste() {
                    Ok(format!("\x1b[200~{clean}\x1b[201~").into_bytes())
                } else {
                    let normalized = clean.replace("\r\n", "\n").replace('\r', "\n");
                    Ok(if self.conpty {
                        normalized.replace('\n', "\r\n")
                    } else {
                        normalized
                    }
                    .into_bytes())
                }
            }
            TerminalInput::Key(event) => encode_key(
                &event.key,
                event.modifiers,
                self.parser.screen().application_cursor(),
            ),
        }
    }

    pub fn snapshot(&mut self) -> TerminalScreen {
        let screen = self.parser.screen_mut();
        screen.set_scrollback(0);
        let (row, column) = screen.cursor_position();
        let cursor = TerminalCursor {
            row,
            column,
            visible: !screen.hide_cursor(),
        };
        let modes = TerminalModes {
            application_cursor: screen.application_cursor(),
            application_keypad: screen.application_keypad(),
            bracketed_paste: screen.bracketed_paste(),
            alternate_screen: screen.alternate_screen(),
        };
        let rows = (0..self.size.rows)
            .map(|row| read_row(screen, row, self.size.columns))
            .collect();
        screen.set_scrollback(usize::MAX);
        let length = screen.scrollback().min(MAX_SCROLLBACK_LINES);
        let mut scrollback = Vec::with_capacity(length);
        for index in (1..=length).rev() {
            screen.set_scrollback(index);
            scrollback.push(read_row(screen, 0, self.size.columns));
        }
        screen.set_scrollback(0);
        TerminalScreen {
            size: self.size,
            rows,
            cursor,
            modes,
            scrollback,
        }
    }
}

fn encode_key(
    key: &TerminalKey,
    modifiers: TerminalModifiers,
    application_cursor: bool,
) -> Result<Vec<u8>, String> {
    if modifiers.super_key {
        return Err("super modifier is not supported by this terminal".into());
    }
    let parameter = 1
        + u8::from(modifiers.shift)
        + 2 * u8::from(modifiers.alt)
        + 4 * u8::from(modifiers.control);
    let modified = parameter != 1;
    let arrow = |letter: char| {
        if modified {
            format!("\x1b[1;{parameter}{letter}").into_bytes()
        } else if application_cursor {
            format!("\x1bO{letter}").into_bytes()
        } else {
            format!("\x1b[{letter}").into_bytes()
        }
    };
    let tilde = |number: u8| {
        if modified {
            format!("\x1b[{number};{parameter}~").into_bytes()
        } else {
            format!("\x1b[{number}~").into_bytes()
        }
    };
    let prefix_alt = |mut bytes: Vec<u8>| {
        if modifiers.alt {
            bytes.insert(0, 27);
        }
        bytes
    };
    Ok(match key {
        TerminalKey::Character(text) => {
            let mut chars = text.chars();
            let c = chars.next().ok_or("empty Terminal key")?;
            if chars.next().is_some() {
                return Err("Terminal key must be one character".into());
            }
            let mut bytes = Vec::new();
            if modifiers.control {
                let code = match c {
                    'a'..='z' => c as u8 - b'a' + 1,
                    'A'..='Z' => c as u8 - b'A' + 1,
                    ' ' | '@' | '2' => 0,
                    '[' | '3' => 27,
                    '\\' | '4' => 28,
                    ']' | '5' => 29,
                    '^' | '6' => 30,
                    '_' | '7' => 31,
                    '?' | '8' => 127,
                    _ => return Err("unsupported control key".into()),
                };
                bytes.push(code);
            } else {
                let mut buffer = [0; 4];
                bytes.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
            }
            prefix_alt(bytes)
        }
        TerminalKey::Enter => prefix_alt(vec![b'\r']),
        TerminalKey::Backspace => prefix_alt(vec![127]),
        TerminalKey::Tab if modifiers.shift && !modifiers.alt && !modifiers.control => {
            b"\x1b[Z".to_vec()
        }
        TerminalKey::Tab if modifiers.control => format!("\x1b[9;{parameter}u").into_bytes(),
        TerminalKey::Tab => prefix_alt(vec![b'\t']),
        TerminalKey::Escape => prefix_alt(vec![27]),
        TerminalKey::ArrowUp => arrow('A'),
        TerminalKey::ArrowDown => arrow('B'),
        TerminalKey::ArrowRight => arrow('C'),
        TerminalKey::ArrowLeft => arrow('D'),
        TerminalKey::Home => arrow('H'),
        TerminalKey::End => arrow('F'),
        TerminalKey::Insert => tilde(2),
        TerminalKey::Delete => tilde(3),
        TerminalKey::PageUp => tilde(5),
        TerminalKey::PageDown => tilde(6),
        TerminalKey::Function(index @ 1..=4) => {
            let letter = (b'P' + index - 1) as char;
            if modified {
                format!("\x1b[1;{parameter}{letter}").into_bytes()
            } else {
                format!("\x1bO{letter}").into_bytes()
            }
        }
        TerminalKey::Function(index @ 5..=12) => {
            tilde([15, 17, 18, 19, 20, 21, 23, 24][usize::from(index - 5)])
        }
        TerminalKey::Function(_) => return Err("unsupported Terminal function key".into()),
    })
}

fn validate(size: TerminalSize) -> Result<(), String> {
    if size.columns == 0 || size.rows == 0 || size.columns > 512 || size.rows > 256 {
        return Err("invalid Terminal size".into());
    }
    Ok(())
}

fn color(value: vt100::Color) -> TerminalColor {
    match value {
        vt100::Color::Default => TerminalColor::Default,
        vt100::Color::Idx(index) => TerminalColor::Indexed(index),
        vt100::Color::Rgb(red, green, blue) => TerminalColor::Rgb { red, green, blue },
    }
}

fn read_row(screen: &vt100::Screen, row: u16, columns: u16) -> TerminalRow {
    let cells = (0..columns)
        .map(|column| {
            let Some(cell) = screen.cell(row, column) else {
                return TerminalCell::default();
            };
            TerminalCell {
                text: if cell.is_wide_continuation() {
                    String::new()
                } else if cell.has_contents() {
                    cell.contents().to_owned()
                } else {
                    " ".into()
                },
                width: if cell.is_wide_continuation() {
                    0
                } else if cell.is_wide() {
                    2
                } else {
                    1
                },
                foreground: color(cell.fgcolor()),
                background: color(cell.bgcolor()),
                bold: cell.bold(),
                italic: cell.italic(),
                underline: cell.underline(),
                dim: cell.dim(),
                inverse: cell.inverse(),
            }
        })
        .collect();
    TerminalRow {
        cells,
        wrapped: screen.row_wrapped(row),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use terminal::TerminalKeyEvent;

    #[test]
    fn encodes_keys_and_paste_using_terminal_modes() {
        let mut renderer = Renderer::new(TerminalSize::new(80, 24)).unwrap();
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
        assert_eq!(
            renderer
                .input(&key(TerminalKey::Character("c".into()), control))
                .unwrap(),
            b"\x03"
        );
        assert_eq!(
            renderer.input(&key(TerminalKey::Backspace, alt)).unwrap(),
            b"\x1b\x7f"
        );
        assert_eq!(
            renderer.input(&key(TerminalKey::Tab, shift)).unwrap(),
            b"\x1b[Z"
        );
        assert_eq!(
            renderer.input(&key(TerminalKey::ArrowUp, control)).unwrap(),
            b"\x1b[1;5A"
        );
        assert_eq!(
            renderer
                .input(&key(TerminalKey::Function(5), control))
                .unwrap(),
            b"\x1b[15;5~"
        );
        assert_eq!(
            renderer
                .input(&TerminalInput::Text("日本語".into()))
                .unwrap(),
            "日本語".as_bytes()
        );
        renderer.process(b"\x1b[?1h\x1b[?2004h");
        assert_eq!(
            renderer
                .input(&key(TerminalKey::Home, Default::default()))
                .unwrap(),
            b"\x1bOH"
        );
        assert_eq!(
            renderer
                .input(&TerminalInput::Paste("paste\x1b[201~".into()))
                .unwrap(),
            b"\x1b[200~paste\x1b[201~"
        );
        assert!(
            renderer
                .input(&key(
                    TerminalKey::Character("ab".into()),
                    Default::default()
                ))
                .is_err()
        );
    }

    #[test]
    fn paste_newlines_follow_pty_type() {
        let input = TerminalInput::Paste("a\nb\rc\r\nd".into());
        let posix = Renderer::with_conpty(TerminalSize::new(80, 24), false).unwrap();
        let windows = Renderer::with_conpty(TerminalSize::new(80, 24), true).unwrap();
        assert_eq!(posix.input(&input).unwrap(), b"a\nb\nc\nd");
        assert_eq!(windows.input(&input).unwrap(), b"a\r\nb\r\nc\r\nd");
    }

    #[test]
    fn renders_styles_wide_characters_modes_and_split_escape_sequences() {
        let mut renderer = Renderer::new(TerminalSize::new(12, 3)).unwrap();
        assert!(renderer.process(b"hello \x1b[31").is_empty());
        assert!(renderer.process("m猫\x1b[?2004h".as_bytes()).is_empty());
        let screen = renderer.snapshot();
        assert!(screen.plain_text().contains("hello 猫"));
        assert_eq!(
            screen.rows[0].cells[6].foreground,
            TerminalColor::Indexed(1)
        );
        assert_eq!(screen.rows[0].cells[6].width, 2);
        assert_eq!(screen.rows[0].cells[7].width, 0);
        assert!(screen.rows[0].cells[7].text.is_empty());
        assert!(screen.modes.bracketed_paste);
    }

    #[test]
    fn replies_to_cursor_queries_and_preserves_scrollback() {
        let mut renderer = Renderer::new(TerminalSize::new(8, 2)).unwrap();
        assert_eq!(renderer.process(b"abc\x1b[6n"), b"\x1b[1;4R");
        renderer.process(b"\r\nline2\r\nline3");
        let screen = renderer.snapshot();
        assert!(
            screen
                .scrollback
                .iter()
                .any(|row| row.plain_text().contains("abc"))
        );
        assert!(screen.plain_text().contains("line3"));
        renderer.resize(TerminalSize::new(10, 3)).unwrap();
        assert_eq!(renderer.snapshot().size, TerminalSize::new(10, 3));
    }
}
