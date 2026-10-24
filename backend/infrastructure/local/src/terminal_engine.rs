use std::{
    io::{self, Write},
    sync::{Arc, mpsc},
};

use lapis_terminal::{
    TerminalCell, TerminalColor, TerminalCursor, TerminalDelta, TerminalError, TerminalInput,
    TerminalKey, TerminalModes, TerminalModifiers, TerminalRevision, TerminalRow,
    TerminalRowUpdate, TerminalScreen, TerminalSize,
};
use wezterm_surface::CursorVisibility;
use wezterm_term::{
    Intensity, KeyCode, KeyModifiers, Line, Terminal, TerminalConfiguration, Underline,
    color::{ColorAttribute, ColorPalette},
};

#[derive(Debug)]
struct Config;

impl TerminalConfiguration for Config {
    fn color_palette(&self) -> ColorPalette {
        ColorPalette::default()
    }
    fn scrollback_size(&self) -> usize {
        2_000
    }
    fn enable_kitty_keyboard(&self) -> bool {
        true
    }
}

// WezTermのwriter threadで発生した失敗を、画面Deltaとは独立してHostへ通知する。
struct PtyWriter {
    inner: Box<dyn Write + Send>,
    failures: mpsc::Sender<String>,
}

impl Write for PtyWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        // upstreamのThreadedWriterはwriteを一度だけ呼ぶため、ここで全byteを保証する。
        match self.inner.write_all(data) {
            Ok(()) => Ok(data.len()),
            Err(error) => {
                let _ = self.failures.send(error.to_string());
                Err(error)
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush().inspect_err(|error| {
            let _ = self.failures.send(error.to_string());
        })
    }
}

pub(crate) struct TerminalEngine {
    term: Terminal,
    failures: mpsc::Receiver<String>,
    size: TerminalSize,
    revision: TerminalRevision,
    last_screen: TerminalScreen,
}

impl TerminalEngine {
    pub(crate) fn new(size: TerminalSize, writer: Box<dyn Write + Send>) -> Self {
        let size = normalized_size(size);
        let (sender, failures) = mpsc::channel();
        let term = Terminal::new(
            engine_size(size),
            Arc::new(Config),
            "Lapis",
            env!("CARGO_PKG_VERSION"),
            Box::new(PtyWriter {
                inner: writer,
                failures: sender,
            }),
        );
        #[cfg(windows)]
        let term = {
            let mut term = term;
            term.enable_conpty_quirks();
            term
        };
        let mut engine = Self {
            term,
            failures,
            size,
            revision: 0,
            last_screen: TerminalScreen::empty(size),
        };
        engine.last_screen = engine.screen();
        engine
    }

    pub(crate) fn process(&mut self, data: &[u8]) -> TerminalDelta {
        self.term.advance_bytes(data);
        self.revision += 1;
        let screen = self.screen();
        let delta = TerminalDelta {
            revision: self.revision,
            size: self.size,
            changed_rows: screen
                .rows
                .iter()
                .enumerate()
                .filter(|&(row, value)| self.last_screen.rows.get(row) != Some(value))
                .map(|(row, value)| TerminalRowUpdate {
                    row: row as u16,
                    value: value.clone(),
                })
                .collect(),
            cursor: screen.cursor,
            modes: screen.modes,
            scrollback: None,
        };
        self.last_screen = screen;
        delta
    }

    pub(crate) fn resize(&mut self, size: TerminalSize) -> (TerminalScreen, TerminalDelta) {
        self.size = normalized_size(size);
        self.term.resize(engine_size(self.size));
        self.revision += 1;
        let screen = self.screen();
        let delta = TerminalDelta {
            revision: self.revision,
            size: self.size,
            changed_rows: screen
                .rows
                .iter()
                .enumerate()
                .map(|(row, value)| TerminalRowUpdate {
                    row: row as u16,
                    value: value.clone(),
                })
                .collect(),
            cursor: screen.cursor,
            modes: screen.modes,
            scrollback: None,
        };
        self.last_screen = screen.clone();
        (screen, delta)
    }

    pub(crate) fn input(&mut self, input: &TerminalInput) -> Result<(), TerminalError> {
        let result = match input {
            TerminalInput::Text(text) => text.chars().try_for_each(|character| {
                self.term
                    .key_down(KeyCode::Char(character), KeyModifiers::NONE)
            }),
            TerminalInput::Paste(text) => self.term.send_paste(text),
            TerminalInput::Key(event) => self
                .term
                .key_down(key_code(&event.key)?, key_modifiers(event.modifiers)),
        };
        result.map_err(|error| TerminalError::new(error.to_string()))
    }

    pub(crate) fn take_failures(&self) -> impl Iterator<Item = String> + '_ {
        self.failures.try_iter()
    }

    fn screen(&self) -> TerminalScreen {
        let mut rows = Vec::with_capacity(self.size.rows as usize);
        let screen = self.term.screen();
        let start = screen
            .scrollback_rows()
            .saturating_sub(self.size.rows as usize);
        screen.with_phys_lines(start..screen.scrollback_rows(), |lines| {
            rows.extend(
                lines
                    .iter()
                    .map(|line| row_from_engine(line, self.size.columns)),
            );
        });
        let cursor = self.term.cursor_pos();
        TerminalScreen {
            size: self.size,
            rows,
            cursor: TerminalCursor {
                row: cursor.y.clamp(0, self.size.rows.saturating_sub(1) as i64) as u16,
                column: cursor.x.min(self.size.columns.saturating_sub(1) as usize) as u16,
                visible: cursor.visibility == CursorVisibility::Visible,
            },
            modes: TerminalModes {
                application_cursor: self.term.application_cursor_keys_enabled(),
                application_keypad: self.term.application_keypad_enabled(),
                bracketed_paste: self.term.bracketed_paste_enabled(),
                alternate_screen: self.term.is_alt_screen_active(),
            },
            scrollback: Vec::new(),
        }
    }
}

pub(crate) fn normalized_size(size: TerminalSize) -> TerminalSize {
    TerminalSize::new(size.columns.max(2), size.rows.max(1))
}

fn engine_size(size: TerminalSize) -> wezterm_term::TerminalSize {
    wezterm_term::TerminalSize {
        rows: size.rows as usize,
        cols: size.columns as usize,
        ..Default::default()
    }
}

fn key_code(key: &TerminalKey) -> Result<KeyCode, TerminalError> {
    Ok(match key {
        TerminalKey::Character(text) => {
            let mut chars = text.chars();
            let character = chars
                .next()
                .ok_or_else(|| TerminalError::new("empty terminal key"))?;
            if chars.next().is_some() {
                return Err(TerminalError::new(
                    "terminal key must be one character; use Text for committed text",
                ));
            }
            KeyCode::Char(character)
        }
        TerminalKey::Enter => KeyCode::Enter,
        TerminalKey::Backspace => KeyCode::Backspace,
        TerminalKey::Tab => KeyCode::Tab,
        TerminalKey::Escape => KeyCode::Escape,
        TerminalKey::ArrowUp => KeyCode::UpArrow,
        TerminalKey::ArrowDown => KeyCode::DownArrow,
        TerminalKey::ArrowLeft => KeyCode::LeftArrow,
        TerminalKey::ArrowRight => KeyCode::RightArrow,
        TerminalKey::Home => KeyCode::Home,
        TerminalKey::End => KeyCode::End,
        TerminalKey::Insert => KeyCode::Insert,
        TerminalKey::Delete => KeyCode::Delete,
        TerminalKey::PageUp => KeyCode::PageUp,
        TerminalKey::PageDown => KeyCode::PageDown,
        TerminalKey::Function(number @ 1..=12) => KeyCode::Function(*number),
        TerminalKey::Function(_) => {
            return Err(TerminalError::new("unsupported terminal function key"));
        }
    })
}

fn key_modifiers(modifiers: TerminalModifiers) -> KeyModifiers {
    let mut result = KeyModifiers::NONE;
    result.set(KeyModifiers::CTRL, modifiers.control);
    result.set(KeyModifiers::ALT, modifiers.alt);
    result.set(KeyModifiers::SHIFT, modifiers.shift);
    result.set(KeyModifiers::SUPER, modifiers.super_key);
    result
}

fn row_from_engine(line: &Line, columns: u16) -> TerminalRow {
    let mut row = TerminalRow::empty(columns);
    row.wrapped = line.last_cell_was_wrapped();
    for cell in line.visible_cells() {
        let column = cell.cell_index();
        if column >= columns as usize {
            break;
        }
        let width = cell.width().min(columns as usize - column).max(1);
        let attrs = cell.attrs();
        row.cells[column] = TerminalCell {
            text: cell.str().to_owned(),
            width: width as u8,
            foreground: color_from_engine(attrs.foreground()),
            background: color_from_engine(attrs.background()),
            bold: attrs.intensity() == Intensity::Bold,
            italic: attrs.italic(),
            underline: attrs.underline() != Underline::None,
            dim: attrs.intensity() == Intensity::Half,
            inverse: attrs.reverse(),
        };
        for spacer in &mut row.cells[column + 1..column + width] {
            spacer.text.clear();
            spacer.width = 0;
        }
    }
    row
}

fn color_from_engine(color: ColorAttribute) -> TerminalColor {
    match color {
        ColorAttribute::Default => TerminalColor::Default,
        ColorAttribute::PaletteIndex(index) => TerminalColor::Indexed(index),
        ColorAttribute::TrueColorWithPaletteFallback(rgb, _)
        | ColorAttribute::TrueColorWithDefaultFallback(rgb) => {
            let (red, green, blue, _) = rgb.to_srgb_u8();
            TerminalColor::Rgb { red, green, blue }
        }
    }
}

#[cfg(test)]
mod tests;
