use std::{
    error::Error,
    fmt,
    ops::Range,
    path::{Path, PathBuf},
};

use lapis_text::TextBuffer;

/// ドキュメントの保存単位。将来の共同編集・競合検知の土台にする。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Revision(u64);

impl Revision {
    pub fn number(self) -> u64 {
        self.0
    }

    fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Encoding {
    #[default]
    Utf8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Document {
    path: Option<PathBuf>,
    text: TextBuffer,
    encoding: Encoding,
    revision: Revision,
    saved_revision: Revision,
}

impl Document {
    pub fn new() -> Self {
        Self {
            path: None,
            text: TextBuffer::new(),
            encoding: Encoding::Utf8,
            revision: Revision::default(),
            saved_revision: Revision::default(),
        }
    }

    pub fn from_file(path: PathBuf, content: String) -> Self {
        let revision = Revision(1);
        Self {
            path: Some(path),
            text: TextBuffer::from_string(content),
            encoding: Encoding::Utf8,
            revision,
            saved_revision: revision,
        }
    }

    pub fn content(&self) -> &str {
        self.text.as_str()
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

    pub fn replace_range(&mut self, range: Range<usize>, replacement: &str) {
        if self.text.replace_range(range, replacement) {
            self.revision = self.revision.next();
        }
    }

    pub fn mark_saved(&mut self, path: PathBuf) {
        self.path = Some(path);
        self.saved_revision = self.revision;
    }

    pub fn is_dirty(&self) -> bool {
        self.revision != self.saved_revision
    }

    pub fn display_name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .unwrap_or("Untitled.md")
            .to_owned()
    }
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

/// ファイルI/Oの失敗を外部実装固有の型から切り離す。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentError {
    message: String,
}

impl DocumentError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
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
    fn read_markdown(&self, path: &Path) -> Result<String, DocumentError>;
    fn write_markdown(&self, path: &Path, content: &str) -> Result<(), DocumentError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revision_changes_when_content_changes() {
        let mut document = Document::new();
        assert!(!document.is_dirty());

        document.replace_range(0..0, "# Lapis");

        assert!(document.is_dirty());
        assert_eq!(document.revision().number(), 1);
        assert_eq!(document.encoding(), Encoding::Utf8);
    }

    #[test]
    fn saving_keeps_current_revision() {
        let mut document = Document::new();
        document.replace_range(0..0, "# Lapis");
        document.mark_saved(PathBuf::from("note.md"));

        assert!(!document.is_dirty());
        assert_eq!(document.display_name(), "note.md");
    }
}
