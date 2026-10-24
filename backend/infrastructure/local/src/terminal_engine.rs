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
///
/// アルゴリズム:
/// 1. 共通prefix（先頭から一致する行）を検出
/// 2. 残りのold suffixとnew suffixを比較
/// 3. 残りのold suffixとnew prefixが一致する部分（重なり）を検出
///
/// 例: old = [A, B, C, D], new = [C, D, E, F]
///   → prefix=0, oldの[C, D] == newの[C, D]（重なり2行）
///   → dropped = 2 (A, B), rows = [E, F]
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

    // 空からあるへ
    if old_len == 0 {
        return Some(TerminalScrollbackDelta {
            reset: false,
            dropped: 0,
            rows: new.to_vec(),
        });
    }
    // あるから空へ
    if new_len == 0 {
        return Some(TerminalScrollbackDelta {
            reset: true,
            dropped: 0,
            rows: Vec::new(),
        });
    }

    // 1. 共通prefixの長さを求める
    let mut prefix = 0;
    let min_len = old_len.min(new_len);
    while prefix < min_len && old[prefix] == new[prefix] {
        prefix += 1;
    }

    // 残りのスライス
    let old_rest = &old[prefix..];
    let new_rest = &new[prefix..];
    let old_rest_len = old_rest.len();
    let new_rest_len = new_rest.len();

    // どちらも残りが空 → 変更なし（すでにold==newでNoneを返している）
    if old_rest_len == 0 && new_rest_len == 0 {
        return None;
    }

    // 2. 純粋なappend: oldの残りが空
    if old_rest_len == 0 {
        return Some(TerminalScrollbackDelta {
            reset: false,
            dropped: 0,
            rows: new_rest.to_vec(),
        });
    }

    // 3. newの残りが空 → oldのprefixがnew全体と一致している
    // 2つのケース:
    //   a) 先頭drop: old = [A, B, C], new = [B, C] → prefix=0, dropped=1
    //      newがoldのsuffixと一致するので、droppedは正しい
    //   b) 末尾truncate: old = [A, B, C], new = [A, B] → prefix=2, dropped=1
    //      newがoldのprefixと一致するので、これは末尾truncate
    //      Client側でdrain(..1)すると[B, C]になって誤り→reset
    if new_rest_len == 0 {
        if prefix == 0 {
            // 先頭drop: newはoldのsuffix。droppedはscrollback先頭から削除する行数
            return Some(TerminalScrollbackDelta {
                reset: false,
                dropped: old_rest_len as u16,
                rows: Vec::new(),
            });
        } else {
            // 末尾truncate: newはoldのprefix。incrementalに表現できない→reset
            return Some(TerminalScrollbackDelta {
                reset: true,
                dropped: 0,
                rows: new.to_vec(),
            });
        }
    }

    // 4. drop + append: old_restのsuffixとnew_restのprefixの重なりを検出
    let max_overlap = old_rest_len.min(new_rest_len);
    let mut overlap = 0;
    for candidate in (1..=max_overlap).rev() {
        if old_rest[old_rest_len - candidate..] == new_rest[..candidate] {
            overlap = candidate;
            break;
        }
    }

    let dropped = old_rest_len - overlap;
    let appended_rows = new_rest[overlap..].to_vec();

    Some(TerminalScrollbackDelta {
        reset: false,
        dropped: dropped as u16,
        rows: appended_rows,
    })
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
