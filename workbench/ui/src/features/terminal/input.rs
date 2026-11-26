use super::TerminalFeature;
use gpui::{
    Bounds, Context, EntityInputHandler, KeyDownEvent, Pixels, Point, UTF16Selection, Window,
};
use std::{collections::VecDeque, ops::Range, time::Duration};
use terminal::{TerminalInput, TerminalKey, TerminalKeyEvent, TerminalModifiers};

/// Text 入力をまとめて送る待ち時間。1 frame 程度に収める。
pub(super) const INPUT_BATCH_DELAY: Duration = Duration::from_millis(8);

/// 端末ごとの送信待ち入力。
///
/// Text は flush まで結合して 1 回で送る。Key / Paste は順序の境界として、
/// 先行する Text を同じ batch に含めて即時 flush する。
#[derive(Default)]
pub(super) struct TerminalInputQueue {
    pending: VecDeque<TerminalInput>,
    in_flight: Option<InFlightBatch>,
    /// pending に Key/Paste が含まれ、待ち時間を置かずに送る必要がある。
    immediate: bool,
}

struct InFlightBatch {
    sequence: u64,
    inputs: Vec<TerminalInput>,
}

pub(super) struct InputBatch {
    pub sequence: u64,
    pub inputs: Vec<TerminalInput>,
}

impl TerminalInputQueue {
    pub(super) fn enqueue_text(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        match self.pending.back_mut() {
            Some(TerminalInput::Text(pending)) => pending.push_str(&text),
            _ => self.pending.push_back(TerminalInput::Text(text)),
        }
    }

    /// Key/Paste を、先行する Text を追い越さない境界として積む。
    pub(super) fn enqueue_immediate(&mut self, input: TerminalInput) {
        debug_assert!(matches!(
            input,
            TerminalInput::Key(_) | TerminalInput::Paste(_)
        ));
        self.pending.push_back(input);
        self.immediate = true;
    }

    pub(super) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub(super) fn has_in_flight(&self) -> bool {
        self.in_flight.is_some()
    }

    pub(super) fn should_flush_now(&self) -> bool {
        self.immediate && !self.pending.is_empty() && self.in_flight.is_none()
    }

    /// 待ち入力を 1 batch にまとめる。送信中は次の batch を作らない。
    pub(super) fn begin_flush(&mut self, sequence: u64) -> Option<InputBatch> {
        if self.in_flight.is_some() || self.pending.is_empty() {
            return None;
        }
        let mut inputs = Vec::with_capacity(self.pending.len());
        for input in self.pending.drain(..) {
            match (inputs.last_mut(), input) {
                (Some(TerminalInput::Text(previous)), TerminalInput::Text(text)) => {
                    previous.push_str(&text);
                }
                (_, input) => inputs.push(input),
            }
        }
        self.immediate = false;
        self.in_flight = Some(InFlightBatch {
            sequence,
            inputs: inputs.clone(),
        });
        Some(InputBatch { sequence, inputs })
    }

    pub(super) fn succeed(&mut self, sequence: u64) -> bool {
        match self.in_flight.take() {
            Some(in_flight) if in_flight.sequence == sequence => true,
            Some(in_flight) => {
                self.in_flight = Some(in_flight);
                false
            }
            None => false,
        }
    }

    /// 失敗した batch のうち受理済みでない入力を先頭へ戻す。
    ///
    /// `applied` 件は受理済みなので再送しない。受理順序を保つため逆順に積む。
    pub(super) fn fail(&mut self, sequence: u64, applied: usize) -> bool {
        let Some(in_flight) = self.in_flight.take() else {
            return false;
        };
        if in_flight.sequence != sequence {
            self.in_flight = Some(in_flight);
            return false;
        }
        let applied = applied.min(in_flight.inputs.len());
        self.immediate |= in_flight.inputs[applied..]
            .iter()
            .any(|input| !matches!(input, TerminalInput::Text(_)));
        for input in in_flight.inputs.into_iter().skip(applied).rev() {
            self.pending.push_front(input);
        }
        true
    }
}

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

    fn key(key: TerminalKey, control: bool) -> TerminalInput {
        TerminalInput::Key(TerminalKeyEvent {
            key,
            modifiers: TerminalModifiers {
                control,
                ..Default::default()
            },
        })
    }

    #[test]
    fn text_is_batched_until_flush_without_loss() {
        let mut queue = TerminalInputQueue::default();
        queue.enqueue_text("a".into());
        queue.enqueue_text("bc".into());
        assert!(!queue.should_flush_now());

        let batch = queue.begin_flush(1).expect("pending text is flushed");
        assert_eq!(batch.inputs, vec![TerminalInput::Text("abc".into())]);
        assert!(queue.begin_flush(2).is_none());
        assert!(queue.succeed(1));
        assert!(!queue.has_pending());
        assert!(!queue.has_in_flight());
    }

    #[test]
    fn text_key_and_paste_keep_their_order_across_flushes() {
        let mut queue = TerminalInputQueue::default();
        queue.enqueue_text("ab".into());
        queue.enqueue_immediate(key(TerminalKey::Enter, false));

        let first = queue.begin_flush(1).expect("key flushes pending text");
        assert_eq!(
            first.inputs,
            vec![
                TerminalInput::Text("ab".into()),
                key(TerminalKey::Enter, false)
            ]
        );

        // 送信中に届いた入力は次の batch へ回り、先行 batch を追い越さない。
        queue.enqueue_text("cd".into());
        queue.enqueue_immediate(TerminalInput::Paste("paste".into()));
        assert!(queue.begin_flush(2).is_none());
        assert!(queue.succeed(1));

        let second = queue.begin_flush(2).expect("queued input keeps order");
        assert_eq!(
            second.inputs,
            vec![
                TerminalInput::Text("cd".into()),
                TerminalInput::Paste("paste".into())
            ]
        );
    }

    #[test]
    fn control_key_is_flushed_immediately_before_pending_text() {
        let mut queue = TerminalInputQueue::default();
        queue.enqueue_text("sleep 1".into());
        assert!(!queue.should_flush_now());

        queue.enqueue_immediate(key(TerminalKey::Character("c".into()), true));
        assert!(queue.should_flush_now());
        let batch = queue.begin_flush(1).expect("control key is not delayed");
        assert_eq!(
            batch.inputs,
            vec![
                TerminalInput::Text("sleep 1".into()),
                key(TerminalKey::Character("c".into()), true)
            ]
        );
    }

    #[test]
    fn failed_batch_requeues_only_unapplied_inputs() {
        let mut queue = TerminalInputQueue::default();
        queue.enqueue_text("ab".into());
        queue.enqueue_immediate(key(TerminalKey::Character("c".into()), true));
        queue.enqueue_immediate(TerminalInput::Paste("paste".into()));

        let _first = queue.begin_flush(7).expect("first batch");
        assert!(queue.fail(7, 1));
        let retry = queue.begin_flush(8).expect("unapplied inputs are retried");
        assert_eq!(
            retry.inputs,
            vec![
                key(TerminalKey::Character("c".into()), true),
                TerminalInput::Paste("paste".into())
            ],
            "applied text must not be sent twice"
        );

        // 再試行も先頭で失敗しても、受理済み件数より後だけを保持する。
        assert!(queue.fail(8, 0));
        let again = queue.begin_flush(9).expect("failed inputs stay queued");
        assert_eq!(again.inputs, retry.inputs);

        assert!(queue.fail(9, 2));
        assert!(!queue.has_pending(), "applied inputs are complete");
    }

    #[test]
    fn stale_batch_result_does_not_consume_the_current_flush() {
        let mut queue = TerminalInputQueue::default();
        queue.enqueue_text("old".into());
        let old = queue.begin_flush(1).unwrap();
        assert!(queue.succeed(1));
        queue.enqueue_text("new".into());
        let current = queue.begin_flush(2).unwrap();

        assert!(!queue.fail(1, 0));
        assert!(!queue.succeed(old.sequence));
        assert!(queue.succeed(current.sequence));
        assert!(!queue.has_pending());
    }

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
