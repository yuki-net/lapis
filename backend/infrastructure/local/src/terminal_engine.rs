use std::sync::{Arc, Mutex};

use alacritty_terminal::{
    event::{Event, EventListener, WindowSize},
    grid::{Dimensions, Grid},
    index::{Column, Line},
    term::{Config, Term, TermMode, cell::Cell, cell::Flags},
    vte::ansi::{Color, NamedColor, Processor},
};
use lapis_terminal::{
    TerminalCell, TerminalColor, TerminalCursor, TerminalDelta, TerminalModes, TerminalRevision,
    TerminalRow, TerminalRowUpdate, TerminalScreen, TerminalSize,
};

const SCROLLBACK_LINES: usize = 2_000;

#[derive(Clone)]
struct ReplyCollector {
    replies: Arc<Mutex<Vec<Vec<u8>>>>,
    size: Arc<Mutex<TerminalSize>>,
}

impl EventListener for ReplyCollector {
    fn send_event(&self, event: Event) {
        let data = match event {
            Event::PtyWrite(data) => Some(data.into_bytes()),
            Event::TextAreaSizeRequest(formatter) => {
                let size = self.size.lock().map(|size| *size).unwrap_or_default();
                Some(
                    formatter(WindowSize {
                        num_lines: size.rows,
                        num_cols: size.columns,
                        cell_width: 0,
                        cell_height: 0,
                    })
                    .into_bytes(),
                )
            }
            Event::ColorRequest(_, _) => None,
            Event::ClipboardLoad(_, _)
            | Event::ClipboardStore(_, _)
            | Event::MouseCursorDirty
            | Event::Title(_)
            | Event::ResetTitle
            | Event::CursorBlinkingChange
            | Event::Wakeup
            | Event::Bell
            | Event::Exit
            | Event::ChildExit(_) => None,
        };
        if let Some(data) = data
            && !data.is_empty()
            && let Ok(mut replies) = self.replies.lock()
        {
            replies.push(data);
        }
    }
}

struct EngineSize {
    columns: usize,
    rows: usize,
}

impl EngineSize {
    fn new(size: TerminalSize) -> Self {
        Self {
            columns: size.columns.max(2) as usize,
            rows: size.rows.max(1) as usize,
        }
    }
}

impl Dimensions for EngineSize {
    fn total_lines(&self) -> usize {
        self.rows + SCROLLBACK_LINES
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

pub(crate) struct TerminalEngine {
    term: Term<ReplyCollector>,
    processor: Processor,
    replies: Arc<Mutex<Vec<Vec<u8>>>>,
    reply_size: Arc<Mutex<TerminalSize>>,
    size: TerminalSize,
    revision: TerminalRevision,
    last_screen: TerminalScreen,
}

impl TerminalEngine {
    pub(crate) fn new(size: TerminalSize) -> Self {
        let size = normalized_size(size);
        let replies = Arc::new(Mutex::new(Vec::new()));
        let reply_size = Arc::new(Mutex::new(size));
        let proxy = ReplyCollector {
            replies: replies.clone(),
            size: reply_size.clone(),
        };
        let term = Term::new(Config::default(), &EngineSize::new(size), proxy);
        let mut engine = Self {
            term,
            processor: Processor::new(),
            replies,
            reply_size,
            size,
            revision: 0,
            last_screen: TerminalScreen::empty(size),
        };
        engine.last_screen = engine.screen();
        engine
    }

    pub(crate) fn process(&mut self, data: &[u8]) -> TerminalDelta {
        self.processor.advance(&mut self.term, data);
        self.revision = self.revision.saturating_add(1);
        let screen = self.screen();
        let changed_rows = screen
            .rows
            .iter()
            .enumerate()
            .filter(|&(row, value)| self.last_screen.rows.get(row) != Some(value))
            .map(|(row, value)| TerminalRowUpdate {
                row: row as u16,
                value: value.clone(),
            })
            .collect();
        let delta = TerminalDelta {
            revision: self.revision,
            size: screen.size,
            changed_rows,
            cursor: screen.cursor,
            modes: screen.modes,
            scrollback: Vec::new(),
        };
        self.last_screen = screen;
        delta
    }

    pub(crate) fn resize(&mut self, size: TerminalSize) -> (TerminalScreen, TerminalDelta) {
        let size = normalized_size(size);
        if let Ok(mut reply_size) = self.reply_size.lock() {
            *reply_size = size;
        }
        self.term.resize(EngineSize::new(size));
        self.size = size;
        self.revision = self.revision.saturating_add(1);
        let screen = self.screen();
        let delta = TerminalDelta {
            revision: self.revision,
            size: screen.size,
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
            scrollback: Vec::new(),
        };
        self.last_screen = screen.clone();
        (screen, delta)
    }

    pub(crate) fn mode(&self) -> TermMode {
        *self.term.mode()
    }

    pub(crate) fn take_replies(&self) -> Vec<Vec<u8>> {
        self.replies
            .lock()
            .map(|mut replies| std::mem::take(&mut *replies))
            .unwrap_or_default()
    }

    fn screen(&self) -> TerminalScreen {
        let grid = self.term.grid();
        let rows = (0..self.size.rows)
            .map(|row| row_from_grid(grid, row as i32, self.size.columns))
            .collect();
        let point = grid.cursor.point;
        let cursor = TerminalCursor {
            row: point
                .line
                .0
                .clamp(0, self.size.rows.saturating_sub(1) as i32) as u16,
            column: point
                .column
                .0
                .min(self.size.columns.saturating_sub(1) as usize) as u16,
            visible: self.term.mode().contains(TermMode::SHOW_CURSOR),
        };
        TerminalScreen {
            size: self.size,
            rows,
            cursor,
            modes: TerminalModes {
                application_cursor: self.term.mode().contains(TermMode::APP_CURSOR),
                application_keypad: self.term.mode().contains(TermMode::APP_KEYPAD),
                bracketed_paste: self.term.mode().contains(TermMode::BRACKETED_PASTE),
                alternate_screen: self.term.mode().contains(TermMode::ALT_SCREEN),
            },
            scrollback: Vec::new(),
        }
    }
}

pub(crate) fn normalized_size(size: TerminalSize) -> TerminalSize {
    TerminalSize::new(size.columns.max(2), size.rows.max(1))
}

fn row_from_grid(grid: &Grid<Cell>, row: i32, columns: u16) -> TerminalRow {
    let line = Line(row);
    let cells = (0..columns as usize)
        .map(|column| cell_from_engine(&grid[line][Column(column)]))
        .collect();
    let wrapped = grid[line][Column(columns.saturating_sub(1) as usize)]
        .flags
        .contains(Flags::WRAPLINE);
    TerminalRow { cells, wrapped }
}

fn cell_from_engine(cell: &Cell) -> TerminalCell {
    let spacer = cell
        .flags
        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER);
    let mut text = if spacer {
        String::new()
    } else {
        cell.c.to_string()
    };
    if !spacer && let Some(zerowidth) = cell.zerowidth() {
        text.extend(zerowidth.iter().copied());
    }
    let (foreground, foreground_dim) = color_from_engine(cell.fg);
    let (background, background_dim) = color_from_engine(cell.bg);
    TerminalCell {
        text,
        width: if spacer {
            0
        } else if cell.flags.contains(Flags::WIDE_CHAR) {
            2
        } else {
            1
        },
        foreground,
        background,
        bold: cell.flags.intersects(Flags::BOLD | Flags::BOLD_ITALIC),
        italic: cell.flags.intersects(Flags::ITALIC | Flags::BOLD_ITALIC),
        underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
        dim: cell.flags.contains(Flags::DIM) || foreground_dim || background_dim,
        inverse: cell.flags.contains(Flags::INVERSE),
    }
}

fn color_from_engine(color: Color) -> (TerminalColor, bool) {
    match color {
        Color::Spec(rgb) => (
            TerminalColor::Rgb {
                red: rgb.r,
                green: rgb.g,
                blue: rgb.b,
            },
            false,
        ),
        Color::Indexed(value) => (TerminalColor::Indexed(value), false),
        Color::Named(named) => match named {
            NamedColor::Foreground
            | NamedColor::BrightForeground
            | NamedColor::Background
            | NamedColor::Cursor => (TerminalColor::Default, false),
            NamedColor::Black => (TerminalColor::Indexed(0), false),
            NamedColor::Red => (TerminalColor::Indexed(1), false),
            NamedColor::Green => (TerminalColor::Indexed(2), false),
            NamedColor::Yellow => (TerminalColor::Indexed(3), false),
            NamedColor::Blue => (TerminalColor::Indexed(4), false),
            NamedColor::Magenta => (TerminalColor::Indexed(5), false),
            NamedColor::Cyan => (TerminalColor::Indexed(6), false),
            NamedColor::White => (TerminalColor::Indexed(7), false),
            NamedColor::BrightBlack => (TerminalColor::Indexed(8), false),
            NamedColor::BrightRed => (TerminalColor::Indexed(9), false),
            NamedColor::BrightGreen => (TerminalColor::Indexed(10), false),
            NamedColor::BrightYellow => (TerminalColor::Indexed(11), false),
            NamedColor::BrightBlue => (TerminalColor::Indexed(12), false),
            NamedColor::BrightMagenta => (TerminalColor::Indexed(13), false),
            NamedColor::BrightCyan => (TerminalColor::Indexed(14), false),
            NamedColor::BrightWhite => (TerminalColor::Indexed(15), false),
            NamedColor::DimBlack => (TerminalColor::Indexed(0), true),
            NamedColor::DimRed => (TerminalColor::Indexed(1), true),
            NamedColor::DimGreen => (TerminalColor::Indexed(2), true),
            NamedColor::DimYellow => (TerminalColor::Indexed(3), true),
            NamedColor::DimBlue => (TerminalColor::Indexed(4), true),
            NamedColor::DimMagenta => (TerminalColor::Indexed(5), true),
            NamedColor::DimCyan => (TerminalColor::Indexed(6), true),
            NamedColor::DimWhite => (TerminalColor::Indexed(7), true),
            NamedColor::DimForeground => (TerminalColor::Default, true),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_keeps_split_utf8_and_combining_cells_in_the_model() {
        let mut engine = TerminalEngine::new(TerminalSize::new(40, 4));
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
        let mut engine = TerminalEngine::new(TerminalSize::new(40, 4));
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
        let mut engine = TerminalEngine::new(TerminalSize::new(40, 4));
        engine.process(b"\x1b[?1049hALT");
        assert!(engine.screen().modes.alternate_screen);

        engine.process(b"\x1b[6n");
        let replies = engine.take_replies();
        assert_eq!(replies, vec![b"\x1b[1;4R".to_vec()]);

        engine.process(b"\x1b[?1049l");
        assert!(!engine.screen().modes.alternate_screen);
    }
}
