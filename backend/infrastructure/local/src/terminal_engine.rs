use std::{
    io::{self, Write},
    sync::{Arc, mpsc},
};

use lapis_terminal::{
    MAX_SCROLLBACK_LINES, TerminalCell, TerminalColor, TerminalCursor, TerminalDelta,
    TerminalError, TerminalInput, TerminalKey, TerminalModes, TerminalModifiers, TerminalRevision,
    TerminalRow, TerminalRowUpdate, TerminalScreen, TerminalScrollbackDelta, TerminalSize,
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

        // scrollbackの差分を内容で計算（length比較だけでは不十分）
        let scrollback_delta =
            compute_scrollback_delta(&self.last_screen.scrollback, &screen.scrollback);

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
            scrollback: scrollback_delta,
        };
        self.last_screen = screen;
        delta
    }

    pub(crate) fn resize(&mut self, size: TerminalSize) -> (TerminalScreen, TerminalDelta) {
        self.size = normalized_size(size);
        self.term.resize(engine_size(self.size));
        self.revision += 1;
        let screen = self.screen();

        // resize時はscrollbackをresetとして再送信
        let scrollback_delta = if !screen.scrollback.is_empty() {
            Some(TerminalScrollbackDelta {
                reset: true,
                dropped: 0,
                rows: screen.scrollback.clone(),
            })
        } else {
            None
        };

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
            scrollback: scrollback_delta,
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
        let mut scrollback = Vec::new();
        let screen = self.term.screen();
        let total_rows = screen.scrollback_rows();

        // visible行の開始位置を計算
        let visible_start = total_rows.saturating_sub(self.size.rows as usize);

        // scrollback行を取得（visible行を除外し、MAX_SCROLLBACK_LINESで制限）
        // WezTermのscrollback_rows()はvisible rowsを含むため、visible_startまでをscrollbackとする
        let scrollback_end = visible_start;
        let scrollback_start = scrollback_end.saturating_sub(MAX_SCROLLBACK_LINES);
        if scrollback_start < scrollback_end {
            screen.with_phys_lines(scrollback_start..scrollback_end, |lines| {
                scrollback.extend(
                    lines
                        .iter()
                        .map(|line| row_from_engine(line, self.size.columns)),
                );
            });
        }

        // 表示行を取得（visible_start..total_rows）
        screen.with_phys_lines(visible_start..total_rows, |lines| {
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
            scrollback,
        }
    }
}

pub(crate) fn normalized_size(size: TerminalSize) -> TerminalSize {
    TerminalSize::new(size.columns.max(2), size.rows.max(1))
}

/// scrollbackの差分を内容で計算する。
/// lengthが同じでも内容が変わっているケース（drop + append）を検出する。
/// 安全に差分化できない場合はresetを返す。
fn compute_scrollback_delta(
    old: &[TerminalRow],
    new: &[TerminalRow],
) -> Option<TerminalScrollbackDelta> {
    if old == new {
        return None;
    }

    let old_len = old.len();
    let new_len = new.len();

    // 両方空なら変更なし
    if old_len == 0 && new_len == 0 {
        return None;
    }

    // 空からあるへ、またはあるから空へ
    if old_len == 0 {
        return Some(TerminalScrollbackDelta {
            reset: false,
            dropped: 0,
            rows: new.to_vec(),
        });
    }
    if new_len == 0 {
        return Some(TerminalScrollbackDelta {
            reset: true,
            dropped: 0,
            rows: Vec::new(),
        });
    }

    // 末尾が一致する部分を探して、drop数とappend数を計算する
    // 例: old = [A, B, C, D], new = [C, D, E, F]
    // → dropped = 2 (A, B), rows = [E, F]

    // 共通接尾辞の長さを求める
    // oldの末尾とnewの末尾が一致する部分を探す
    let mut common_suffix = 0;
    let min_len = old_len.min(new_len);
    while common_suffix < min_len
        && old[old_len - 1 - common_suffix] == new[new_len - 1 - common_suffix]
    {
        common_suffix += 1;
    }

    // 共通接尾辞の手前が一致しているか確認
    // oldの共通分の手前とnewの共通分の手前が一致すれば、純粋なappend/drop
    let old_prefix_len = old_len - common_suffix;
    let new_prefix_len = new_len - common_suffix;

    if old_prefix_len == new_prefix_len {
        // 長さが同じで内容も同じ → 変更なし（すでにold == newでNoneを返している）
        // ここには来ない
        return Some(TerminalScrollbackDelta {
            reset: true,
            dropped: 0,
            rows: new.to_vec(),
        });
    }

    let dropped = old_prefix_len.max(new_prefix_len) - old_prefix_len.min(new_prefix_len);

    // newの末尾からcommon_suffix分を除いたものが追加された行
    let appended_start = new_len - common_suffix;
    let appended_rows = if appended_start < new_len {
        new[appended_start..].to_vec()
    } else {
        Vec::new()
    };

    // dropped > 0 かつ appended_rows が空でない、またはその逆
    // 安全に差分化できるか確認
    if dropped + appended_rows.len() == old_prefix_len.max(new_prefix_len) {
        Some(TerminalScrollbackDelta {
            reset: false,
            dropped: dropped as u16,
            rows: appended_rows,
        })
    } else {
        // 安全に差分化できない場合はreset
        Some(TerminalScrollbackDelta {
            reset: true,
            dropped: 0,
            rows: new.to_vec(),
        })
    }
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
