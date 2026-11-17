//! Web でも使用できる Terminal の出力描画器。

use terminal::{
    MAX_SCROLLBACK_LINES, TerminalCell, TerminalColor, TerminalCursor, TerminalModes, TerminalRow,
    TerminalScreen, TerminalSize,
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
}

impl Renderer {
    pub fn new(size: TerminalSize) -> Result<Self, String> {
        validate(size)?;
        Ok(Self {
            parser: vt100::Parser::new_with_callbacks(
                size.rows,
                size.columns,
                MAX_SCROLLBACK_LINES,
                Replies::default(),
            ),
            size,
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
                text: if cell.has_contents() {
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
