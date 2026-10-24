use super::TerminalFeature;
use gpui::{
    Bounds, Context, EntityInputHandler, KeyDownEvent, Pixels, Point, UTF16Selection, Window,
};
use lapis_terminal::{TerminalInput, TerminalKey, TerminalKeyEvent, TerminalModifiers};
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
        if let Some(event) = semantic_key(key) {
            self.send_input(TerminalInput::Key(event), cx);
            cx.stop_propagation();
        }
    }
}

fn semantic_key(key: &gpui::Keystroke) -> Option<TerminalKeyEvent> {
    let name = key.key.to_ascii_lowercase();
    let terminal_key = match name.as_str() {
        "enter" => TerminalKey::Enter,
        "backspace" => TerminalKey::Backspace,
        "tab" => TerminalKey::Tab,
        "escape" => TerminalKey::Escape,
        "up" => TerminalKey::ArrowUp,
        "down" => TerminalKey::ArrowDown,
        "left" => TerminalKey::ArrowLeft,
        "right" => TerminalKey::ArrowRight,
        "home" => TerminalKey::Home,
        "end" => TerminalKey::End,
        "insert" => TerminalKey::Insert,
        "delete" => TerminalKey::Delete,
        "pageup" => TerminalKey::PageUp,
        "pagedown" => TerminalKey::PageDown,
        "f1" => TerminalKey::Function(1),
        "f2" => TerminalKey::Function(2),
        "f3" => TerminalKey::Function(3),
        "f4" => TerminalKey::Function(4),
        "f5" => TerminalKey::Function(5),
        "f6" => TerminalKey::Function(6),
        "f7" => TerminalKey::Function(7),
        "f8" => TerminalKey::Function(8),
        "f9" => TerminalKey::Function(9),
        "f10" => TerminalKey::Function(10),
        "f11" => TerminalKey::Function(11),
        "f12" => TerminalKey::Function(12),
        _ => {
            // 通常文字とShiftのみの文字はGPUIのtext/IME入力へ渡す。
            if !(key.modifiers.control || key.modifiers.alt || key.modifiers.platform) {
                return None;
            }
            let text = key.key_char.clone().or_else(|| {
                let mut characters = key.key.chars();
                (characters.next().is_some() && characters.next().is_none())
                    .then(|| key.key.clone())
            })?;
            TerminalKey::Character(text)
        }
    };
    Some(TerminalKeyEvent {
        key: terminal_key,
        modifiers: TerminalModifiers {
            shift: key.modifiers.shift,
            control: key.modifiers.control,
            alt: key.modifiers.alt,
            super_key: key.modifiers.platform,
        },
    })
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
    fn printable_keys_remain_on_the_gpui_text_and_ime_path() {
        for text in ["a", "A", " ", "あ"] {
            for shift in [false, true] {
                let mut key = gpui::Keystroke {
                    key: text.to_owned(),
                    key_char: Some(text.to_owned()),
                    ..Default::default()
                };
                key.modifiers.shift = shift;
                assert_eq!(semantic_key(&key), None);
            }
        }
    }

    #[test]
    fn terminal_keys_are_sent_as_semantic_events() {
        let mut key = gpui::Keystroke {
            key: "c".to_owned(),
            ..Default::default()
        };
        key.modifiers.control = true;
        assert_eq!(
            semantic_key(&key),
            Some(TerminalKeyEvent {
                key: TerminalKey::Character("c".to_owned()),
                modifiers: TerminalModifiers {
                    control: true,
                    ..Default::default()
                },
            })
        );
        key.modifiers.control = false;
        key.key = "up".to_owned();
        assert_eq!(
            semantic_key(&key).map(|event| event.key),
            Some(TerminalKey::ArrowUp)
        );
        key.key = "tab".to_owned();
        key.modifiers.shift = true;
        assert_eq!(
            semantic_key(&key).map(|event| event.modifiers.shift),
            Some(true)
        );
    }

    #[test]
    fn terminal_ctrl_space_keeps_the_character_and_modifier_meaning() {
        let mut key = gpui::Keystroke {
            key: "space".to_owned(),
            key_char: Some(" ".to_owned()),
            ..Default::default()
        };
        key.modifiers.control = true;
        assert_eq!(
            semantic_key(&key),
            Some(TerminalKeyEvent {
                key: TerminalKey::Character(" ".to_owned()),
                modifiers: TerminalModifiers {
                    control: true,
                    ..Default::default()
                },
            })
        );
    }
}
