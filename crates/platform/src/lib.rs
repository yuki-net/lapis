use std::{fs, path::Path};

use lapis_app_services::MarkdownFileDialog;
use lapis_document::{DocumentError, DocumentRepository};

#[derive(Default)]
pub struct LocalDocumentRepository;

impl DocumentRepository for LocalDocumentRepository {
    fn read_markdown(&self, path: &Path) -> Result<String, DocumentError> {
        fs::read_to_string(path).map_err(|error| DocumentError::new(error.to_string()))
    }

    fn write_markdown(&self, path: &Path, content: &str) -> Result<(), DocumentError> {
        fs::write(path, content).map_err(|error| DocumentError::new(error.to_string()))
    }
}

#[derive(Default)]
pub struct NativeMarkdownFileDialog;

impl MarkdownFileDialog for NativeMarkdownFileDialog {
    fn choose_open_path(&self) -> Option<std::path::PathBuf> {
        rfd::FileDialog::new()
            .add_filter("Markdown", &["md", "markdown", "mdown"])
            .pick_file()
    }

    fn choose_save_path(&self, suggested_name: &str) -> Option<std::path::PathBuf> {
        rfd::FileDialog::new()
            .add_filter("Markdown", &["md", "markdown", "mdown"])
            .set_file_name(suggested_name)
            .save_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_repository_round_trips_markdown() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        let repository = LocalDocumentRepository;

        repository.write_markdown(&path, "# Hello\n").unwrap();

        assert_eq!(repository.read_markdown(&path).unwrap(), "# Hello\n");
    }
}
