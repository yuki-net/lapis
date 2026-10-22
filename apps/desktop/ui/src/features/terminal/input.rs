use super::TerminalFeature;
use gpui::{
    Bounds, Context, EntityInputHandler, KeyDownEvent, Pixels, Point, UTF16Selection, Window,
};
use std::ops::Range;

impl TerminalFeature {
    pub(super) fn key_down(
        &mut self,
        event: &KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = &event.keystroke;
        if key.modifiers.control && key.key.eq_ignore_ascii_case("v") {
            self.send_clipboard(cx);
            cx.stop_propagation();
            return;
        }
        if !self.marked.is_empty() {
            return;
        }
        let application_cursor = self
            .active_screen()
            .is_some_and(|screen| screen.application_cursor());
        if let Some(bytes) = key_bytes(key, application_cursor) {
            self.send_text(bytes, cx);
            cx.stop_propagation();
        }
    }
}

fn key_bytes(key: &gpui::Keystroke, application_cursor: bool) -> Option<String> {
    let text = key.key.as_str();
    if key.modifiers.control && (text == "space" || text == " ") {
        return Some("\0".to_owned());
    }
    if key.modifiers.control && text.len() == 1 {
        let byte = text.as_bytes()[0].to_ascii_uppercase();
        if (b'@'..=b'_').contains(&byte) {
            return Some(char::from(byte & 0x1f).to_string());
        }
    }
    let arrow = match text {
        "up" => Some('A'),
        "down" => Some('B'),
        "right" => Some('C'),
        "left" => Some('D'),
        _ => None,
    };
    if let Some(arrow) = arrow {
        let modifier = 1
            + u8::from(key.modifiers.shift)
            + 2 * u8::from(key.modifiers.alt)
            + 4 * u8::from(key.modifiers.control);
        return Some(if modifier != 1 {
            format!("\x1b[1;{modifier}{arrow}")
        } else if application_cursor {
            format!("\x1bO{arrow}")
        } else {
            format!("\x1b[{arrow}")
        });
    }
    let bytes = match text {
        "enter" => "\r",
        "backspace" => "\x7f",
        "escape" => "\x1b",
        "tab" if key.modifiers.shift => "\x1b[Z",
        "tab" => "\t",
        "home" => "\x1b[H",
        "end" => "\x1b[F",
        "insert" => "\x1b[2~",
        "delete" => "\x1b[3~",
        "pageup" => "\x1b[5~",
        "pagedown" => "\x1b[6~",
        "f1" => "\x1bOP",
        "f2" => "\x1bOQ",
        "f3" => "\x1bOR",
        "f4" => "\x1bOS",
        "f5" => "\x1b[15~",
        "f6" => "\x1b[17~",
        "f7" => "\x1b[18~",
        "f8" => "\x1b[19~",
        "f9" => "\x1b[20~",
        "f10" => "\x1b[21~",
        "f11" => "\x1b[23~",
        "f12" => "\x1b[24~",
        _ if key.modifiers.alt => return key.key_char.as_ref().map(|text| format!("\x1b{text}")),
        _ => return None,
    };
    Some(bytes.to_owned())
}

impl EntityInputHandler for TerminalFeature {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let utf16 = self.marked.encode_utf16().collect::<Vec<_>>();
        let range = range.start.min(utf16.len())..range.end.min(utf16.len());
        actual.replace(range.clone());
        Some(String::from_utf16_lossy(&utf16[range]))
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let end = self.marked.encode_utf16().count();
        Some(UTF16Selection {
            range: end..end,
            reversed: false,
        })
    }
    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        (!self.marked.is_empty()).then(|| 0..self.marked.encode_utf16().count())
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked.clear();
        cx.notify();
    }
    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked.clear();
        self.send_text(text.to_owned(), cx);
        cx.notify();
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = text.to_owned();
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(bounds)
    }
    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_keys_preserve_interrupt_tab_and_application_cursor_sequences() {
        let mut key = gpui::Keystroke {
            key: "c".to_owned(),
            ..Default::default()
        };
        key.modifiers.control = true;
        assert_eq!(key_bytes(&key, false).as_deref(), Some("\x03"));
        key.modifiers.control = false;
        key.key = "up".to_owned();
        assert_eq!(key_bytes(&key, true).as_deref(), Some("\x1bOA"));
        key.modifiers.control = true;
        assert_eq!(key_bytes(&key, true).as_deref(), Some("\x1b[1;5A"));
        key.modifiers.control = false;
        key.key = "tab".to_owned();
        assert_eq!(key_bytes(&key, false).as_deref(), Some("\t"));
    }

    #[test]
    fn terminal_ctrl_space_sends_nul_for_gpui_space_names() {
        let mut key = gpui::Keystroke {
            key: "space".to_owned(),
            ..Default::default()
        };
        key.modifiers.control = true;
        assert_eq!(key_bytes(&key, false).as_deref(), Some("\0"));
        key.key = " ".to_owned();
        assert_eq!(key_bytes(&key, false).as_deref(), Some("\0"));
    }
}
