use std::{
    error::Error,
    fmt,
    ops::Range,
    path::{Path, PathBuf},
};

pub use lapis_text::{Position, TextRange};
use lapis_text::{TextBuffer, TextEdit, TextError};

/// Document内で単調増加する変更世代。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Revision(u64);

impl Revision {
    pub const fn number(self) -> u64 {
        self.0
    }

    fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Encoding {
    #[default]
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileFingerprint {
    pub byte_len: u64,
    pub modified_nanos: Option<u128>,
    pub content_hash: u64,
}

impl FileFingerprint {
    pub const fn new(byte_len: u64, modified_nanos: Option<u128>, content_hash: u64) -> Self {
        Self {
            byte_len,
            modified_nanos,
            content_hash,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileData {
    pub bytes: Vec<u8>,
    pub fingerprint: FileFingerprint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalChange {
    Unchanged,
    Modified,
    Deleted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Document {
    path: Option<PathBuf>,
    text: TextBuffer,
    encoding: Encoding,
    revision: Revision,
    saved_revision: Revision,
    saved_fingerprint: Option<FileFingerprint>,
}

impl Document {
    pub fn new() -> Self {
        Self {
            path: None,
            text: TextBuffer::new(),
            encoding: Encoding::Utf8,
            revision: Revision::default(),
            saved_revision: Revision::default(),
            saved_fingerprint: None,
        }
    }

    pub fn from_file(path: PathBuf, data: FileData) -> Result<Self, DocumentError> {
        let (content, encoding) = decode_text(&data.bytes)?;
        let revision = Revision(1);
        Ok(Self {
            path: Some(path),
            text: TextBuffer::from_string(content),
            encoding,
            revision,
            saved_revision: revision,
            saved_fingerprint: Some(data.fingerprint),
        })
    }

    /// まだ保存されていない、保存先を持つ新規Documentを作る。
    pub fn draft(path: PathBuf, content: String, encoding: Encoding) -> Self {
        Self {
            path: Some(path),
            text: TextBuffer::from_string(content),
            encoding,
            revision: Revision(1),
            saved_revision: Revision::default(),
            saved_fingerprint: None,
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
        self.text.is_empty()
    }

    pub fn content_for_save(&self) -> String {
        self.text.to_contiguous_string()
    }

    pub fn line(&self, line: usize) -> Option<String> {
        self.text.line(line)
    }

    pub fn slice_chars(&self, range: Range<usize>) -> Result<String, DocumentError> {
        self.text.slice_chars(range).map_err(DocumentError::from)
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn revision(&self) -> Revision {
        self.revision
    }

    pub fn encoding(&self) -> Encoding {
        self.encoding
    }

    pub fn saved_fingerprint(&self) -> Option<&FileFingerprint> {
        self.saved_fingerprint.as_ref()
    }

    pub fn char_to_byte(&self, char_index: usize) -> Result<usize, DocumentError> {
        self.text
            .char_to_byte(char_index)
            .map_err(DocumentError::from)
    }

    pub fn byte_to_char(&self, byte_index: usize) -> Result<usize, DocumentError> {
        self.text
            .byte_to_char(byte_index)
            .map_err(DocumentError::from)
    }

    pub fn char_to_position(&self, char_index: usize) -> Result<Position, DocumentError> {
        self.text
            .char_to_position(char_index)
            .map_err(DocumentError::from)
    }

    pub fn position_to_char(&self, position: Position) -> Result<usize, DocumentError> {
        self.text
            .position_to_char(position)
            .map_err(DocumentError::from)
    }

    pub fn char_to_utf16_offset(&self, char_index: usize) -> Result<usize, DocumentError> {
        self.text
            .char_to_utf16_offset(char_index)
            .map_err(DocumentError::from)
    }

    pub fn utf16_offset_to_char(&self, offset: usize) -> Result<usize, DocumentError> {
        self.text
            .utf16_offset_to_char(offset)
            .map_err(DocumentError::from)
    }

    pub fn replace_char_range(
        &mut self,
        range: Range<usize>,
        replacement: &str,
    ) -> Result<bool, DocumentError> {
        let changed = self
            .text
            .replace_char_range(range, replacement)
            .map_err(DocumentError::from)?;
        if changed {
            self.revision = self.revision.next();
        }
        Ok(changed)
    }

    pub fn apply_transaction(&mut self, edits: Vec<TextEdit>) -> Result<bool, DocumentError> {
        let changed = self
            .text
            .apply_transaction(edits)
            .map_err(DocumentError::from)?;
        if changed {
            self.revision = self.revision.next();
        }
        Ok(changed)
    }

    pub fn undo(&mut self) -> bool {
        if self.text.undo() {
            self.revision = self.revision.next();
            true
        } else {
            false
        }
    }

    pub fn redo(&mut self) -> bool {
        if self.text.redo() {
            self.revision = self.revision.next();
            true
        } else {
            false
        }
    }

    pub fn can_undo(&self) -> bool {
        self.text.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.text.can_redo()
    }

    pub fn find(&self, query: &str, case_sensitive: bool) -> Vec<Range<usize>> {
        self.text.find(query, case_sensitive)
    }

    pub fn encoded_bytes(&self) -> Vec<u8> {
        encode_text(&self.text.to_contiguous_string(), self.encoding)
    }

    pub fn mark_saved(&mut self, path: PathBuf, fingerprint: FileFingerprint) {
        self.path = Some(path);
        self.saved_revision = self.revision;
        self.saved_fingerprint = Some(fingerprint);
    }

    pub fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }

    pub fn display_name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .unwrap_or("Untitled")
            .to_owned()
    }

    pub fn external_change(&self, actual: Option<&FileFingerprint>) -> ExternalChange {
        match (&self.saved_fingerprint, actual) {
            (Some(_), None) => ExternalChange::Deleted,
            (Some(expected), Some(actual)) if expected != actual => ExternalChange::Modified,
            _ => ExternalChange::Unchanged,
        }
    }
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentErrorKind {
    Io,
    InvalidEncoding,
    Conflict,
    InvalidRange,
}

/// ファイルI/Oや文字コードの失敗を外部実装固有の型から切り離す。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentError {
    kind: DocumentErrorKind,
    message: String,
}

impl DocumentError {
    pub fn new(kind: DocumentErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn io(message: impl Into<String>) -> Self {
        Self::new(DocumentErrorKind::Io, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(DocumentErrorKind::Conflict, message)
    }

    pub const fn kind(&self) -> DocumentErrorKind {
        self.kind
    }
}

impl From<TextError> for DocumentError {
    fn from(error: TextError) -> Self {
        Self::new(DocumentErrorKind::InvalidRange, error.to_string())
    }
}

impl fmt::Display for DocumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for DocumentError {}

/// Document機能側が所有する永続化契約。
pub trait DocumentRepository: Send + Sync {
    fn read_file(&self, path: &Path) -> Result<FileData, DocumentError>;

    fn write_file(
        &self,
        path: &Path,
        content: &[u8],
        expected: Option<&FileFingerprint>,
    ) -> Result<FileFingerprint, DocumentError>;

    fn fingerprint(&self, path: &Path) -> Result<Option<FileFingerprint>, DocumentError>;
}

fn decode_text(bytes: &[u8]) -> Result<(String, Encoding), DocumentError> {
    if let Some(content) = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        return String::from_utf8(content.to_vec())
            .map(|text| (text, Encoding::Utf8Bom))
            .map_err(|error| {
                DocumentError::new(DocumentErrorKind::InvalidEncoding, error.to_string())
            });
    }
    if let Some(content) = bytes.strip_prefix(&[0xff, 0xfe]) {
        return decode_utf16(content, true).map(|text| (text, Encoding::Utf16Le));
    }
    if let Some(content) = bytes.strip_prefix(&[0xfe, 0xff]) {
        return decode_utf16(content, false).map(|text| (text, Encoding::Utf16Be));
    }
    String::from_utf8(bytes.to_vec())
        .map(|text| (text, Encoding::Utf8))
        .map_err(|error| DocumentError::new(DocumentErrorKind::InvalidEncoding, error.to_string()))
}

fn decode_utf16(bytes: &[u8], little_endian: bool) -> Result<String, DocumentError> {
    if !bytes.len().is_multiple_of(2) {
        return Err(DocumentError::new(
            DocumentErrorKind::InvalidEncoding,
            "UTF-16ファイルのbyte数が奇数です",
        ));
    }
    let units = bytes.chunks_exact(2).map(|pair| {
        let value = [pair[0], pair[1]];
        if little_endian {
            u16::from_le_bytes(value)
        } else {
            u16::from_be_bytes(value)
        }
    });
    char::decode_utf16(units)
        .collect::<Result<String, _>>()
        .map_err(|error| DocumentError::new(DocumentErrorKind::InvalidEncoding, error.to_string()))
}

fn encode_text(text: &str, encoding: Encoding) -> Vec<u8> {
    match encoding {
        Encoding::Utf8 => text.as_bytes().to_vec(),
        Encoding::Utf8Bom => [0xef, 0xbb, 0xbf]
            .into_iter()
            .chain(text.as_bytes().iter().copied())
            .collect(),
        Encoding::Utf16Le | Encoding::Utf16Be => {
            let mut bytes = if encoding == Encoding::Utf16Le {
                vec![0xff, 0xfe]
            } else {
                vec![0xfe, 0xff]
            };
            for unit in text.encode_utf16() {
                let pair = if encoding == Encoding::Utf16Le {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                };
                bytes.extend_from_slice(&pair);
            }
            bytes
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fingerprint(bytes: &[u8]) -> FileFingerprint {
        FileFingerprint::new(bytes.len() as u64, Some(1), 42)
    }

    #[test]
    fn revision_changes_for_edit_undo_and_redo() {
        let mut document = Document::new();
        document.replace_char_range(0..0, "# Lapis").unwrap();
        assert_eq!(document.revision().number(), 1);
        assert!(document.undo());
        assert_eq!(document.revision().number(), 2);
        assert!(document.redo());
        assert_eq!(document.revision().number(), 3);
        assert_eq!(document.content_for_save(), "# Lapis");
    }

    #[test]
    fn utf16_round_trip_preserves_bom_and_unicode() {
        let original = encode_text("日本😀\r\n", Encoding::Utf16Le);
        let document = Document::from_file(
            PathBuf::from("note.txt"),
            FileData {
                fingerprint: fingerprint(&original),
                bytes: original.clone(),
            },
        )
        .unwrap();
        assert_eq!(document.encoding(), Encoding::Utf16Le);
        assert_eq!(document.encoded_bytes(), original);
    }

    #[test]
    fn external_changes_are_not_silently_ignored() {
        let bytes = b"hello";
        let document = Document::from_file(
            PathBuf::from("note.md"),
            FileData {
                fingerprint: fingerprint(bytes),
                bytes: bytes.to_vec(),
            },
        )
        .unwrap();
        let changed = FileFingerprint::new(6, Some(2), 99);
        assert_eq!(
            document.external_change(Some(&changed)),
            ExternalChange::Modified
        );
        assert_eq!(document.external_change(None), ExternalChange::Deleted);
    }

    #[test]
    fn draft_keeps_requested_encoding_and_starts_dirty() {
        let document = Document::draft(
            PathBuf::from("new.ts"),
            "const value = 1;".to_owned(),
            Encoding::Utf8Bom,
        );

        assert!(document.is_dirty());
        assert_eq!(document.revision().number(), 1);
        assert_eq!(document.encoding(), Encoding::Utf8Bom);
        assert!(document.encoded_bytes().starts_with(&[0xef, 0xbb, 0xbf]));
    }
}
