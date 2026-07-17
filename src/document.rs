use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// ドキュメントの保存単位。Revision は将来の共同編集・競合検知の土台にする。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Revision {
    pub number: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Document {
    pub path: Option<PathBuf>,
    pub content: String,
    pub revision: Revision,
    pub saved_revision: u64,
}

impl Document {
    pub fn new() -> Self {
        Self {
            path: None,
            content: String::new(),
            revision: Revision { number: 0 },
            saved_revision: 0,
        }
    }

    pub fn from_file(path: PathBuf, content: String) -> Self {
        Self {
            path: Some(path),
            content,
            revision: Revision { number: 1 },
            saved_revision: 1,
        }
    }

    pub fn set_content(&mut self, content: String) {
        if self.content != content {
            self.content = content;
            self.revision.number += 1;
        }
    }

    pub fn mark_saved(&mut self, path: PathBuf) {
        self.path = Some(path);
        self.saved_revision = self.revision.number;
    }

    pub fn is_dirty(&self) -> bool {
        self.revision.number != self.saved_revision
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

/// UI から直接ファイルシステムを触らせないためのローカル Workspace 境界。
pub struct WorkspaceBackend;

impl WorkspaceBackend {
    pub fn read_markdown(path: &Path) -> io::Result<String> {
        fs::read_to_string(path)
    }

    pub fn write_markdown(path: &Path, content: &str) -> io::Result<()> {
        fs::write(path, content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revision_changes_when_content_changes() {
        let mut document = Document::new();
        assert!(!document.is_dirty());

        document.set_content("# Lapis".to_owned());

        assert!(document.is_dirty());
        assert_eq!(document.revision.number, 1);
    }

    #[test]
    fn backend_round_trips_markdown() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");

        WorkspaceBackend::write_markdown(&path, "# Hello\n").unwrap();

        assert_eq!(WorkspaceBackend::read_markdown(&path).unwrap(), "# Hello\n");
    }
}
