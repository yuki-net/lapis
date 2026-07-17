use std::{ops::Range, path::PathBuf, sync::Arc};

use lapis_document::{Document, DocumentError, DocumentRepository};

/// ネイティブファイルダイアログを利用するユースケース側の契約。
pub trait MarkdownFileDialog: Send + Sync {
    fn choose_open_path(&self) -> Option<PathBuf>;
    fn choose_save_path(&self, suggested_name: &str) -> Option<PathBuf>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentAction {
    Completed,
    Cancelled,
}

/// UIからDocumentと外部I/Oを分離する、ローカル編集セッション。
pub struct EditorSession {
    document: Document,
    repository: Arc<dyn DocumentRepository>,
    file_dialog: Arc<dyn MarkdownFileDialog>,
}

impl EditorSession {
    pub fn new(
        repository: Arc<dyn DocumentRepository>,
        file_dialog: Arc<dyn MarkdownFileDialog>,
    ) -> Self {
        Self {
            document: Document::new(),
            repository,
            file_dialog,
        }
    }

    pub fn content(&self) -> &str {
        self.document.content()
    }

    pub fn display_name(&self) -> String {
        self.document.display_name()
    }

    pub fn revision(&self) -> u64 {
        self.document.revision().number()
    }

    pub fn is_dirty(&self) -> bool {
        self.document.is_dirty()
    }

    pub fn replace_range(&mut self, range: Range<usize>, replacement: &str) {
        self.document.replace_range(range, replacement);
    }

    pub fn new_document(&mut self) {
        self.document = Document::new();
    }

    pub fn open_document(&mut self) -> Result<DocumentAction, DocumentError> {
        let Some(path) = self.file_dialog.choose_open_path() else {
            return Ok(DocumentAction::Cancelled);
        };
        let content = self.repository.read_markdown(&path)?;
        self.document = Document::from_file(path, content);
        Ok(DocumentAction::Completed)
    }

    pub fn save_document(&mut self) -> Result<DocumentAction, DocumentError> {
        let path = if let Some(path) = self.document.path() {
            path.to_owned()
        } else {
            let Some(path) = self
                .file_dialog
                .choose_save_path(&self.document.display_name())
            else {
                return Ok(DocumentAction::Cancelled);
            };
            path
        };
        self.repository
            .write_markdown(&path, self.document.content())?;
        self.document.mark_saved(path);
        Ok(DocumentAction::Completed)
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, path::Path, sync::Mutex};

    use super::*;

    #[derive(Default)]
    struct MemoryRepository(Mutex<HashMap<PathBuf, String>>);

    impl DocumentRepository for MemoryRepository {
        fn read_markdown(&self, path: &Path) -> Result<String, DocumentError> {
            self.0
                .lock()
                .unwrap()
                .get(path)
                .cloned()
                .ok_or_else(|| DocumentError::new("not found"))
        }

        fn write_markdown(&self, path: &Path, content: &str) -> Result<(), DocumentError> {
            self.0
                .lock()
                .unwrap()
                .insert(path.to_owned(), content.to_owned());
            Ok(())
        }
    }

    struct FixedDialog {
        open_path: Option<PathBuf>,
        save_path: Option<PathBuf>,
    }

    impl MarkdownFileDialog for FixedDialog {
        fn choose_open_path(&self) -> Option<PathBuf> {
            self.open_path.clone()
        }

        fn choose_save_path(&self, _suggested_name: &str) -> Option<PathBuf> {
            self.save_path.clone()
        }
    }

    #[test]
    fn save_routes_through_injected_boundaries() {
        let repository = Arc::new(MemoryRepository::default());
        let path = PathBuf::from("note.md");
        let dialog = Arc::new(FixedDialog {
            open_path: None,
            save_path: Some(path.clone()),
        });
        let mut session = EditorSession::new(repository.clone(), dialog);
        session.replace_range(0..0, "# Lapis\n");

        assert_eq!(session.save_document().unwrap(), DocumentAction::Completed);
        assert_eq!(
            repository.0.lock().unwrap().get(&path).unwrap(),
            "# Lapis\n"
        );
        assert!(!session.is_dirty());
    }

    #[test]
    fn cancelled_open_keeps_the_current_document() {
        let repository = Arc::new(MemoryRepository::default());
        let dialog = Arc::new(FixedDialog {
            open_path: None,
            save_path: None,
        });
        let mut session = EditorSession::new(repository, dialog);
        session.replace_range(0..0, "draft");

        assert_eq!(session.open_document().unwrap(), DocumentAction::Cancelled);
        assert_eq!(session.content(), "draft");
    }

    #[test]
    fn open_replaces_the_document_with_repository_content() {
        let repository = Arc::new(MemoryRepository::default());
        let path = PathBuf::from("opened.md");
        repository
            .0
            .lock()
            .unwrap()
            .insert(path.clone(), "# Opened\n".to_owned());
        let dialog = Arc::new(FixedDialog {
            open_path: Some(path),
            save_path: None,
        });
        let mut session = EditorSession::new(repository, dialog);

        assert_eq!(session.open_document().unwrap(), DocumentAction::Completed);
        assert_eq!(session.content(), "# Opened\n");
        assert_eq!(session.display_name(), "opened.md");
        assert!(!session.is_dirty());
    }
}
