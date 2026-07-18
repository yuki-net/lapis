use std::{error::Error, fmt, ops::Range};

use ropey::Rope;

/// LSPなどUTF-16基準の外部境界でも安全に扱える0始まりの行列位置。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Position {
    pub line: u32,
    pub utf16_column: u32,
}

impl Position {
    pub const fn new(line: u32, utf16_column: u32) -> Self {
        Self { line, utf16_column }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextRange {
    pub start: Position,
    pub end: Position,
}

impl TextRange {
    pub const fn new(start: Position, end: Position) -> Self {
        Self { start, end }
    }
}

/// Transactionへ渡す編集。rangeは編集前のUnicode scalar index。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub range: Range<usize>,
    pub replacement: String,
}

impl TextEdit {
    pub fn new(range: Range<usize>, replacement: impl Into<String>) -> Self {
        Self {
            range,
            replacement: replacement.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextError {
    message: String,
}

impl TextError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for TextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for TextError {}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AppliedEdit {
    final_start: usize,
    deleted: String,
    inserted: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Transaction {
    original: Vec<TextEdit>,
    applied: Vec<AppliedEdit>,
}

/// UIやファイルI/Oに依存しないropeベースのテキストバッファ。
#[derive(Clone, Debug, Default)]
pub struct TextBuffer {
    text: Rope,
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
}

impl PartialEq for TextBuffer {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
    }
}

impl Eq for TextBuffer {}

impl TextBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_string(content: String) -> Self {
        Self {
            text: Rope::from_str(&content),
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn len_chars(&self) -> usize {
        self.text.len_chars()
    }

    pub fn len_bytes(&self) -> usize {
        self.text.len_bytes()
    }

    pub fn len_lines(&self) -> usize {
        self.text.len_lines()
    }

    pub fn is_empty(&self) -> bool {
        self.text.len_chars() == 0
    }

    /// ファイル保存など連続領域が必要な境界でのみ全文を生成する。
    pub fn to_contiguous_string(&self) -> String {
        self.text.to_string()
    }

    pub fn line(&self, line: usize) -> Option<String> {
        self.text.get_line(line).map(|value| value.to_string())
    }

    pub fn slice_chars(&self, range: Range<usize>) -> Result<String, TextError> {
        self.validate_char_range(&range)?;
        Ok(self.text.slice(range).to_string())
    }

    pub fn char_to_byte(&self, char_index: usize) -> Result<usize, TextError> {
        if char_index > self.text.len_chars() {
            return Err(TextError::new("文字位置が文書範囲外です"));
        }
        Ok(self.text.char_to_byte(char_index))
    }

    pub fn byte_to_char(&self, byte_index: usize) -> Result<usize, TextError> {
        if byte_index > self.text.len_bytes() {
            return Err(TextError::new("byte位置が文書範囲外です"));
        }
        let char_index = self.text.byte_to_char(byte_index);
        if self.text.char_to_byte(char_index) != byte_index {
            return Err(TextError::new("byte位置がUTF-8文字境界ではありません"));
        }
        Ok(char_index)
    }

    pub fn char_to_position(&self, char_index: usize) -> Result<Position, TextError> {
        if char_index > self.text.len_chars() {
            return Err(TextError::new("文字位置が文書範囲外です"));
        }
        let line = self.text.char_to_line(char_index);
        let line_start = self.text.line_to_char(line);
        let utf16_column = self
            .text
            .slice(line_start..char_index)
            .chars()
            .map(char::len_utf16)
            .sum::<usize>();
        Ok(Position::new(
            u32::try_from(line).map_err(|_| TextError::new("行番号が大きすぎます"))?,
            u32::try_from(utf16_column)
                .map_err(|_| TextError::new("UTF-16列番号が大きすぎます"))?,
        ))
    }

    pub fn position_to_char(&self, position: Position) -> Result<usize, TextError> {
        let line =
            usize::try_from(position.line).map_err(|_| TextError::new("行番号を変換できません"))?;
        if line >= self.text.len_lines() {
            return Err(TextError::new("行番号が文書範囲外です"));
        }
        let target = usize::try_from(position.utf16_column)
            .map_err(|_| TextError::new("UTF-16列番号を変換できません"))?;
        let line_start = self.text.line_to_char(line);
        let mut utf16 = 0usize;
        for (offset, ch) in self.text.line(line).chars().enumerate() {
            if utf16 == target {
                return Ok(line_start + offset);
            }
            if ch == '\n' || ch == '\r' {
                break;
            }
            utf16 += ch.len_utf16();
            if utf16 > target {
                return Err(TextError::new(
                    "UTF-16列番号がサロゲートペアの途中を指しています",
                ));
            }
        }
        if utf16 == target {
            Ok(line_start
                + self
                    .text
                    .line(line)
                    .chars()
                    .take_while(|ch| *ch != '\n' && *ch != '\r')
                    .count())
        } else {
            Err(TextError::new("UTF-16列番号が行範囲外です"))
        }
    }

    pub fn char_to_utf16_offset(&self, char_index: usize) -> Result<usize, TextError> {
        if char_index > self.text.len_chars() {
            return Err(TextError::new("文字位置が文書範囲外です"));
        }
        Ok(self
            .text
            .slice(..char_index)
            .chars()
            .map(char::len_utf16)
            .sum())
    }

    pub fn utf16_offset_to_char(&self, offset: usize) -> Result<usize, TextError> {
        let mut utf16 = 0usize;
        for (char_index, ch) in self.text.chars().enumerate() {
            if utf16 == offset {
                return Ok(char_index);
            }
            utf16 += ch.len_utf16();
            if utf16 > offset {
                return Err(TextError::new(
                    "UTF-16位置がサロゲートペアの途中を指しています",
                ));
            }
        }
        if utf16 == offset {
            Ok(self.text.len_chars())
        } else {
            Err(TextError::new("UTF-16位置が文書範囲外です"))
        }
    }

    pub fn replace_char_range(
        &mut self,
        range: Range<usize>,
        replacement: &str,
    ) -> Result<bool, TextError> {
        self.apply_transaction(vec![TextEdit::new(range, replacement)])
    }

    pub fn replace_byte_range(
        &mut self,
        range: Range<usize>,
        replacement: &str,
    ) -> Result<bool, TextError> {
        let start = self.byte_to_char(range.start)?;
        let end = self.byte_to_char(range.end)?;
        self.replace_char_range(start..end, replacement)
    }

    pub fn apply_transaction(&mut self, mut edits: Vec<TextEdit>) -> Result<bool, TextError> {
        if edits.is_empty() {
            return Ok(false);
        }
        edits.sort_by_key(|edit| edit.range.start);
        let mut previous_end = 0usize;
        for edit in &edits {
            self.validate_char_range(&edit.range)?;
            if edit.range.start < previous_end {
                return Err(TextError::new("同一Transactionの編集範囲が重複しています"));
            }
            previous_end = edit.range.end;
        }

        let mut changed = false;
        let mut shift: isize = 0;
        let mut applied = Vec::with_capacity(edits.len());
        for edit in &edits {
            let deleted = self.text.slice(edit.range.clone()).to_string();
            if deleted != edit.replacement {
                changed = true;
            }
            let final_start = edit
                .range
                .start
                .checked_add_signed(shift)
                .ok_or_else(|| TextError::new("編集位置の計算が範囲外です"))?;
            applied.push(AppliedEdit {
                final_start,
                deleted,
                inserted: edit.replacement.clone(),
            });
            shift += edit.replacement.chars().count() as isize
                - (edit.range.end - edit.range.start) as isize;
        }
        if !changed {
            return Ok(false);
        }

        for edit in edits.iter().rev() {
            self.text.remove(edit.range.clone());
            self.text.insert(edit.range.start, &edit.replacement);
        }
        self.undo.push(Transaction {
            original: edits,
            applied,
        });
        self.redo.clear();
        Ok(true)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) -> bool {
        let Some(transaction) = self.undo.pop() else {
            return false;
        };
        for edit in transaction.applied.iter().rev() {
            let inserted_end = edit.final_start + edit.inserted.chars().count();
            self.text.remove(edit.final_start..inserted_end);
            self.text.insert(edit.final_start, &edit.deleted);
        }
        self.redo.push(transaction);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(transaction) = self.redo.pop() else {
            return false;
        };
        for edit in transaction.original.iter().rev() {
            self.text.remove(edit.range.clone());
            self.text.insert(edit.range.start, &edit.replacement);
        }
        self.undo.push(transaction);
        true
    }

    pub fn find(&self, query: &str, case_sensitive: bool) -> Vec<Range<usize>> {
        if query.is_empty() {
            return Vec::new();
        }
        let text = self.text.to_string();
        if case_sensitive {
            return text
                .match_indices(query)
                .map(|(start_byte, matched)| {
                    let end_byte = start_byte + matched.len();
                    let start = text[..start_byte].chars().count();
                    let end = start + text[start_byte..end_byte].chars().count();
                    start..end
                })
                .collect();
        }

        let needle = query.to_lowercase();
        let chars = text.chars().collect::<Vec<_>>();
        let mut matches = Vec::new();
        for start in 0..chars.len() {
            let mut candidate = String::new();
            for (relative_end, ch) in chars[start..].iter().enumerate() {
                candidate.extend(ch.to_lowercase());
                if candidate == needle {
                    matches.push(start..start + relative_end + 1);
                    break;
                }
                if !needle.starts_with(&candidate) {
                    break;
                }
            }
        }
        matches
    }

    fn validate_char_range(&self, range: &Range<usize>) -> Result<(), TextError> {
        if range.start > range.end || range.end > self.text.len_chars() {
            Err(TextError::new("編集範囲が文書範囲外です"))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transaction_and_undo_redo_preserve_unicode() {
        let mut buffer = TextBuffer::from_string("a日本😀z".to_owned());
        assert!(buffer.replace_char_range(1..4, "🦀").unwrap());
        assert_eq!(buffer.to_contiguous_string(), "a🦀z");
        assert!(buffer.undo());
        assert_eq!(buffer.to_contiguous_string(), "a日本😀z");
        assert!(buffer.redo());
        assert_eq!(buffer.to_contiguous_string(), "a🦀z");
    }

    #[test]
    fn utf16_positions_reject_surrogate_middle() {
        let buffer = TextBuffer::from_string("日本😀\r\nnext".to_owned());
        let emoji_end = buffer.position_to_char(Position::new(0, 4)).unwrap();
        assert_eq!(
            buffer.char_to_position(emoji_end).unwrap(),
            Position::new(0, 4)
        );
        assert!(buffer.position_to_char(Position::new(0, 3)).is_err());
        assert_eq!(buffer.position_to_char(Position::new(1, 4)).unwrap(), 9);
    }

    #[test]
    fn byte_ranges_must_be_utf8_boundaries() {
        let mut buffer = TextBuffer::from_string("a😀b".to_owned());
        assert!(buffer.replace_byte_range(1..5, "x").unwrap());
        assert_eq!(buffer.to_contiguous_string(), "axb");

        let buffer = TextBuffer::from_string("a😀b".to_owned());
        assert!(buffer.byte_to_char(2).is_err());
    }

    #[test]
    fn multiple_non_overlapping_edits_form_one_undo_step() {
        let mut buffer = TextBuffer::from_string("abcdef".to_owned());
        buffer
            .apply_transaction(vec![TextEdit::new(1..2, "XX"), TextEdit::new(4..5, "Y")])
            .unwrap();
        assert_eq!(buffer.to_contiguous_string(), "aXXcdYf");
        assert!(buffer.undo());
        assert_eq!(buffer.to_contiguous_string(), "abcdef");
        assert!(buffer.redo());
        assert_eq!(buffer.to_contiguous_string(), "aXXcdYf");
    }

    #[test]
    fn search_returns_character_ranges() {
        let buffer = TextBuffer::from_string("日本語 日本語".to_owned());
        assert_eq!(buffer.find("日本", true), vec![0..2, 4..6]);

        let buffer = TextBuffer::from_string("Straße STRASSE".to_owned());
        assert_eq!(buffer.find("straße", false), vec![0..6]);
    }

    #[test]
    fn large_document_middle_edits_stay_on_rope_operations() {
        let content = "0123456789日本😀\n".repeat(300_000);
        let mut buffer = TextBuffer::from_string(content);
        let original_len = buffer.len_chars();
        for _ in 0..200 {
            let middle = buffer.len_chars() / 2;
            buffer.replace_char_range(middle..middle, "x").unwrap();
        }
        assert_eq!(buffer.len_chars(), original_len + 200);
        for _ in 0..200 {
            assert!(buffer.undo());
        }
        assert_eq!(buffer.len_chars(), original_len);
    }
}
