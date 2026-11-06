//! IMEの置換対象と選択状態を計算する。文書への反映成功後にだけ状態を確定する。
use lapis_document::{Document, DocumentError, DocumentErrorKind};
use std::ops::Range;

pub(super) enum InputKind {
    Commit,
    Mark(Option<Range<usize>>),
}

pub(super) struct InputEdit {
    pub(super) target: Range<usize>,
    pub(super) selection: Range<usize>,
    pub(super) marked: Option<Range<usize>>,
}

impl InputEdit {
    pub(super) fn new(
        document: &Document,
        selected: &Range<usize>,
        marked: Option<&Range<usize>>,
        explicit_utf16: Option<Range<usize>>,
        text: &str,
        kind: InputKind,
    ) -> Result<Self, DocumentError> {
        // 確定も候補更新も、明示範囲 → 変換中範囲 → 通常選択の順で置換する。
        let target = if let Some(range) = explicit_utf16 {
            document.utf16_offset_to_char(range.start)?..document.utf16_offset_to_char(range.end)?
        } else {
            marked.unwrap_or(selected).clone()
        };
        if target.start > target.end || target.end > document.len_chars() {
            return Err(invalid_range());
        }
        let end = target.start + text.chars().count();
        let (selection, marked) = match kind {
            InputKind::Commit => (end..end, None),
            InputKind::Mark(selection) => {
                let selection = match selection {
                    Some(range) => {
                        if range.start > range.end {
                            return Err(invalid_range());
                        }
                        target.start + char_for_utf16(text, range.start)?
                            ..target.start + char_for_utf16(text, range.end)?
                    }
                    None => end..end,
                };
                (selection, (!text.is_empty()).then_some(target.start..end))
            }
        };
        Ok(Self {
            target,
            selection,
            marked,
        })
    }
}

fn invalid_range() -> DocumentError {
    DocumentError::new(
        DocumentErrorKind::InvalidRange,
        "IMEの置換・選択範囲が不正です",
    )
}

fn char_for_utf16(text: &str, target: usize) -> Result<usize, DocumentError> {
    let mut utf16 = 0;
    let mut chars = 0;
    for ch in text.chars() {
        if utf16 == target {
            return Ok(chars);
        }
        utf16 += ch.len_utf16();
        chars += 1;
        if utf16 > target {
            return Err(invalid_range());
        }
    }
    if utf16 == target {
        Ok(chars)
    } else {
        Err(invalid_range())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Input {
        document: Document,
        selected: Range<usize>,
        marked: Option<Range<usize>>,
    }

    impl Input {
        fn new(text: &str, selected: Range<usize>) -> Self {
            let mut document = Document::new();
            document.replace_char_range(0..0, text).unwrap();
            Self {
                document,
                selected,
                marked: None,
            }
        }

        fn apply(&mut self, explicit: Option<Range<usize>>, text: &str, kind: InputKind) {
            let plan = InputEdit::new(
                &self.document,
                &self.selected,
                self.marked.as_ref(),
                explicit,
                text,
                kind,
            )
            .unwrap();
            self.document.replace_char_range(plan.target, text).unwrap();
            self.selected = plan.selection;
            self.marked = plan.marked;
        }
    }

    #[test]
    fn japanese_candidates_and_commit_replace_the_whole_preedit_once() {
        let mut input = Input::new("prefix suffix", 7..7);
        input.apply(None, "にほんご", InputKind::Mark(None));
        input.apply(None, "日本語", InputKind::Mark(Some(0..2)));
        assert_eq!(input.selected, 7..9);
        input.apply(None, "日本語", InputKind::Commit);
        assert_eq!(input.document.content_for_save(), "prefix 日本語suffix");
        assert_eq!(input.selected, 10..10);
        assert_eq!(input.marked, None);
        input.apply(None, "!", InputKind::Commit);
        assert_eq!(input.document.content_for_save(), "prefix 日本語!suffix");
    }

    #[test]
    fn cancellation_removes_preedit_and_empty_mark_is_not_retained() {
        let mut input = Input::new("abc", 1..2);
        input.apply(None, "かんじ", InputKind::Mark(None));
        input.apply(None, "", InputKind::Commit);
        assert_eq!(input.document.content_for_save(), "ac");
        assert_eq!(input.marked, None);
        input.apply(None, "", InputKind::Mark(None));
        assert_eq!(input.marked, None);
    }

    #[test]
    fn utf16_prefix_emoji_crlf_and_explicit_ranges() {
        let mut input = Input::new("😀\r\nabc", 3..6);
        input.apply(None, "日😀本", InputKind::Mark(Some(1..3)));
        assert_eq!(input.marked, Some(3..6));
        assert_eq!(input.selected, 4..5);
        input.apply(None, "日本", InputKind::Commit);
        assert_eq!(input.document.content_for_save(), "😀\r\n日本");
        input.apply(None, "仮", InputKind::Mark(None));
        input.apply(Some(0..2), "先頭", InputKind::Commit);
        assert_eq!(input.document.content_for_save(), "先頭\r\n日本仮");
        assert_eq!(input.marked, None);
    }

    #[test]
    fn repeated_preedit_with_same_text_can_change_clause_selection() {
        let mut input = Input::new("", 0..0);
        input.apply(None, "日本語", InputKind::Mark(Some(0..1)));
        input.apply(None, "日本語", InputKind::Mark(Some(1..3)));
        assert_eq!(input.document.content_for_save(), "日本語");
        assert_eq!(input.selected, 1..3);
        input.apply(None, "日本語", InputKind::Commit);
        assert_eq!(input.document.content_for_save(), "日本語");
    }

    #[test]
    fn invalid_utf16_or_reversed_ranges_do_not_produce_an_edit() {
        let input = Input::new("😀", 0..0);
        assert!(
            InputEdit::new(
                &input.document,
                &input.selected,
                None,
                Some(1..2),
                "x",
                InputKind::Commit
            )
            .is_err()
        );
        assert!(
            InputEdit::new(
                &input.document,
                &input.selected,
                None,
                None,
                "😀",
                InputKind::Mark(Some(0..1))
            )
            .is_err()
        );
        assert!(
            InputEdit::new(
                &input.document,
                &input.selected,
                None,
                Some(Range { start: 2, end: 0 }),
                "x",
                InputKind::Commit
            )
            .is_err()
        );
    }
}
