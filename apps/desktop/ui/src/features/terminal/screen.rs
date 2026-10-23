use lapis_terminal::TerminalSnapshot;

pub(super) struct TerminalScreen {
    parser: vt100::Parser,
    output_end: u64,
    initialized: bool,
    size: (u16, u16),
}

impl TerminalScreen {
    pub fn new(snapshot: &TerminalSnapshot) -> Self {
        let mut screen = Self {
            parser: vt100::Parser::new(snapshot.rows.max(1), snapshot.columns.max(1), 2000),
            output_end: 0,
            initialized: false,
            size: (snapshot.rows, snapshot.columns),
        };
        screen.update(snapshot);
        screen
    }

    pub fn update(&mut self, snapshot: &TerminalSnapshot) {
        if self.size != (snapshot.rows, snapshot.columns) {
            self.parser
                .screen_mut()
                .set_size(snapshot.rows.max(1), snapshot.columns.max(1));
            self.size = (snapshot.rows, snapshot.columns);
        }
        let end = snapshot
            .output_offset
            .saturating_add(snapshot.output.len() as u64);
        if !self.initialized || self.output_end < snapshot.output_offset || self.output_end > end {
            // Hostの画面を起点に、snapshot以降のraw outputだけを適用する。
            self.parser = vt100::Parser::new(snapshot.rows.max(1), snapshot.columns.max(1), 2000);
            self.output_end = snapshot.output_offset;
            if let Some(screen) = &snapshot.screen
                && (snapshot.output_offset..=end).contains(&screen.output_end)
            {
                self.parser.process(&screen.data);
                self.output_end = screen.output_end;
            }
        }
        self.parser
            .process(&snapshot.output[(self.output_end - snapshot.output_offset) as usize..]);
        self.output_end = end;
        self.initialized = true;
    }

    pub fn rows(&self) -> Vec<String> {
        self.parser.screen().rows(0, self.size.1).collect()
    }

    pub fn application_cursor(&self) -> bool {
        self.parser.screen().application_cursor()
    }

    pub fn bracketed_paste(&self) -> bool {
        self.parser.screen().bracketed_paste()
    }

    pub fn cursor(&self) -> Option<(u16, u16)> {
        (!self.parser.screen().hide_cursor()).then(|| self.parser.screen().cursor_position())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lapis_terminal::{TerminalId, TerminalStatus};

    #[test]
    fn carriage_returns_cursor_moves_and_split_utf8_render_as_a_screen() {
        let mut snapshot = TerminalSnapshot {
            id: TerminalId::new("test"),
            cwd: "test".into(),
            status: TerminalStatus::Running,
            rows: 3,
            columns: 20,
            output: b"progress 10%\rprogress 20%\x1b[K\r\n\xe3\x81".to_vec(),
            output_sequence: 1,
            output_truncated: false,
            output_offset: 0,
            screen: None,
        };
        let mut screen = TerminalScreen::new(&snapshot);
        snapshot.output.extend(b"\x82\x1b[1;1Hdone\x1b[K");
        snapshot.output_sequence += 1;
        screen.update(&snapshot);
        assert_eq!(screen.rows()[0], "done");
        assert_eq!(screen.rows()[1], "あ");
        assert!(!screen.rows().iter().any(|line| line.contains('\x1b')));
    }
}
