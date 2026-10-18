//! UI検証用のメモリー内workspace。OS・外部プロセスには接続しない。
use std::{
    collections::{BTreeMap, BTreeSet},
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use lapis_app_services::{
    ConversationRecord, ConversationRepository, WorkspaceDialog, WorkspaceRepository,
};
use lapis_document::{
    DocumentError, DocumentErrorKind, DocumentRepository, FileData, FileFingerprint, Revision,
};
use lapis_editor_core::{ConversationId, ExecutionId, TaskId};
use lapis_git::{FileDiff, GitBackend, GitError, GitErrorKind, RepositoryStatus, TaskWorktree};
use lapis_lsp::{
    CompletionItem, DefinitionTarget, Diagnostic, LanguageServerBackend, LspError, LspPosition,
};
use lapis_settings::{GlobalSettings, GlobalSettingsRepository, SettingsError};
use lapis_task_runner::{TaskBackend, TaskControl, TaskError, TaskRecord};
use lapis_terminal::{TerminalBackend, TerminalError, TerminalEvent, TerminalId};
use lapis_workspace::{
    FileEntry, FileEntryKind, SearchError, SearchHit, WorkspaceError, WorkspaceSearchBackend,
    WorkspaceSnapshot, WorkspaceStateRepository,
};

pub(super) const ROOT: &str = "/Lapis Web";
const UNAVAILABLE: &str = "Web prototype: native backend is not connected";

#[derive(Default)]
pub(super) struct PrototypeBackend {
    files: Mutex<BTreeMap<PathBuf, Vec<u8>>>,
    snapshot: Mutex<Option<WorkspaceSnapshot>>,
    settings: Mutex<GlobalSettings>,
    conversations: Mutex<(Vec<ConversationRecord>, Option<ConversationId>)>,
}

impl PrototypeBackend {
    pub(super) fn new() -> Self {
        let backend = Self::default();
        for (name, content) in [
            (
                "README.md",
                "# Lapis Web prototype\n\nThe same Rust/GPUI views as Desktop, running in your browser.\n\n## Try it\n\n- Edit text, select, undo and redo.\n- Open files from the file tree and switch tabs.\n- Ctrl+Shift+K: command palette.\n- Ctrl+F: search in the editor.\n- Ctrl+J: toggle the bottom panel.\n- Toggle Markdown preview and change the theme.\n- Resize panels and the browser window.\n\n## Scope\n\nFiles and settings live in memory. Reload resets this workspace.\nTerminal, Git, LSP and external runners are not connected.\nThis is a GPUI WebAssembly prototype, not a remote desktop stream.\n\n日本語入力・表示も検証できます。\n",
            ),
            (
                "src/main.rs",
                "// Lapis: the Desktop editor, compiled to WebAssembly.\n// Try editing this sample. Changes stay in browser memory.\n\nfn main() {\n    let platform = \"GPUI Web\";\n    println!(\"Hello from {platform}!\");\n}\n",
            ),
            (
                "notes.md",
                "# Notes\n\nUse this file to try multiple tabs and Markdown preview.\n\n- [ ] Test keyboard input\n- [ ] Test panel resizing\n- [ ] Try a different theme\n",
            ),
        ] {
            backend
                .files
                .lock()
                .unwrap()
                .insert(Path::new(ROOT).join(name), content.as_bytes().to_vec());
        }
        backend
    }
}

fn fingerprint(bytes: &[u8]) -> FileFingerprint {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    FileFingerprint::new(bytes.len() as u64, None, hasher.finish())
}

impl DocumentRepository for PrototypeBackend {
    fn read_file(&self, path: &Path) -> Result<FileData, DocumentError> {
        let files = self.files.lock().unwrap();
        let bytes = files.get(path).ok_or_else(|| {
            DocumentError::new(DocumentErrorKind::Io, "Sample file was not found")
        })?;
        Ok(FileData {
            bytes: bytes.clone(),
            fingerprint: fingerprint(bytes),
        })
    }

    fn write_file(
        &self,
        path: &Path,
        content: &[u8],
        expected: Option<&FileFingerprint>,
    ) -> Result<FileFingerprint, DocumentError> {
        if !path.starts_with(ROOT)
            || path
                .components()
                .any(|part| part == std::path::Component::ParentDir)
        {
            return Err(DocumentError::new(
                DocumentErrorKind::Io,
                "Only the prototype workspace can be saved",
            ));
        }
        let mut files = self.files.lock().unwrap();
        if let Some(expected) = expected
            && files.get(path).map(|bytes| fingerprint(bytes)).as_ref() != Some(expected)
        {
            return Err(DocumentError::new(
                DocumentErrorKind::Conflict,
                "The in-memory file changed",
            ));
        }
        files.insert(path.to_owned(), content.to_vec());
        Ok(fingerprint(content))
    }

    fn fingerprint(&self, path: &Path) -> Result<Option<FileFingerprint>, DocumentError> {
        Ok(self
            .files
            .lock()
            .unwrap()
            .get(path)
            .map(|bytes| fingerprint(bytes)))
    }
}

impl WorkspaceRepository for PrototypeBackend {
    fn list_tree(&self, root: &Path) -> Result<Vec<FileEntry>, WorkspaceError> {
        if root != Path::new(ROOT) {
            return Err(WorkspaceError::new("Open the sample workspace"));
        }
        let files = self.files.lock().unwrap();
        let mut entries = BTreeMap::new();
        for path in files.keys() {
            let relative = path
                .strip_prefix(root)
                .map_err(|error| WorkspaceError::new(error.to_string()))?;
            let mut parents = BTreeSet::new();
            let mut parent = relative.parent();
            while let Some(directory) = parent.filter(|path| !path.as_os_str().is_empty()) {
                parents.insert(directory.to_owned());
                parent = directory.parent();
            }
            for relative in parents {
                entries.insert(
                    relative.clone(),
                    FileEntry {
                        path: root.join(&relative),
                        depth: relative.components().count() - 1,
                        relative_path: relative,
                        kind: FileEntryKind::Directory,
                    },
                );
            }
            entries.insert(
                relative.to_owned(),
                FileEntry {
                    path: path.clone(),
                    depth: relative.components().count() - 1,
                    relative_path: relative.to_owned(),
                    kind: FileEntryKind::File,
                },
            );
        }
        Ok(entries.into_values().collect())
    }
}

impl WorkspaceDialog for PrototypeBackend {
    fn choose_workspace_path(&self) -> Option<PathBuf> {
        Some(ROOT.into())
    }
    fn choose_file_path(&self) -> Option<PathBuf> {
        Some(Path::new(ROOT).join("README.md"))
    }
    fn choose_save_path(&self, suggested_name: &str) -> Option<PathBuf> {
        Some(Path::new(ROOT).join(suggested_name))
    }
}

impl WorkspaceStateRepository for PrototypeBackend {
    fn load(&self) -> Result<Option<WorkspaceSnapshot>, WorkspaceError> {
        Ok(self.snapshot.lock().unwrap().clone())
    }
    fn save(&self, snapshot: &WorkspaceSnapshot) -> Result<(), WorkspaceError> {
        *self.snapshot.lock().unwrap() = Some(snapshot.clone());
        Ok(())
    }
}

impl ConversationRepository for PrototypeBackend {
    fn load(&self) -> Result<(Vec<ConversationRecord>, Option<ConversationId>), WorkspaceError> {
        Ok(self.conversations.lock().unwrap().clone())
    }
    fn save(
        &self,
        records: &[ConversationRecord],
        active: &ConversationId,
    ) -> Result<(), WorkspaceError> {
        *self.conversations.lock().unwrap() = (records.to_vec(), Some(active.clone()));
        Ok(())
    }
}

impl GlobalSettingsRepository for PrototypeBackend {
    fn load(&self) -> Result<GlobalSettings, SettingsError> {
        Ok(self.settings.lock().unwrap().clone())
    }
    fn save(&self, settings: &GlobalSettings) -> Result<(), SettingsError> {
        *self.settings.lock().unwrap() = settings.clone();
        Ok(())
    }
}

impl WorkspaceSearchBackend for PrototypeBackend {
    fn search(
        &self,
        root: &Path,
        query: &str,
        cancelled: &AtomicBool,
    ) -> Result<Vec<SearchHit>, SearchError> {
        let query = query.to_ascii_lowercase();
        let mut hits = Vec::new();
        for (path, bytes) in self
            .files
            .lock()
            .unwrap()
            .iter()
            .filter(|(path, _)| path.starts_with(root))
        {
            for (line, text) in String::from_utf8_lossy(bytes).lines().enumerate() {
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(Vec::new());
                }
                if let Some(column) = text.to_ascii_lowercase().find(&query) {
                    hits.push(SearchHit {
                        path: path.clone(),
                        line,
                        utf8_column: column,
                        preview: text.into(),
                    });
                }
            }
        }
        Ok(hits)
    }
}

impl TaskBackend for PrototypeBackend {
    fn load(&self) -> Result<Vec<TaskRecord>, TaskError> {
        Ok(Vec::new())
    }
    fn start(&self, _: &TaskRecord) -> Result<(), TaskError> {
        Err(TaskError::new(UNAVAILABLE))
    }
    fn control(&self, _: &ExecutionId, _: &TaskControl) -> Result<(), TaskError> {
        Err(TaskError::new(UNAVAILABLE))
    }
}

impl GitBackend for PrototypeBackend {
    fn status(&self, _: &Path) -> Result<RepositoryStatus, GitError> {
        Err(GitError::new(GitErrorKind::NotRepository, UNAVAILABLE))
    }
    fn diff(&self, _: &Path, _: &Path) -> Result<FileDiff, GitError> {
        Err(GitError::new(GitErrorKind::NotRepository, UNAVAILABLE))
    }
    fn create_worktree(&self, _: &Path, _: &TaskId) -> Result<TaskWorktree, GitError> {
        Err(GitError::new(GitErrorKind::NotRepository, UNAVAILABLE))
    }
    fn worktrees(&self, _: &Path) -> Result<Vec<TaskWorktree>, GitError> {
        Ok(Vec::new())
    }
    fn import_file(&self, _: &TaskWorktree, _: &Path) -> Result<TaskWorktree, GitError> {
        Err(GitError::new(GitErrorKind::NotRepository, UNAVAILABLE))
    }
    fn discard_worktree(&self, _: &TaskWorktree) -> Result<TaskWorktree, GitError> {
        Err(GitError::new(GitErrorKind::NotRepository, UNAVAILABLE))
    }
}

impl LanguageServerBackend for PrototypeBackend {
    fn start(&self, _: &Path) -> Result<(), LspError> {
        Err(LspError::new(UNAVAILABLE))
    }
    fn did_open(&self, _: &Path, _: &str, _: Revision) -> Result<(), LspError> {
        Err(LspError::new(UNAVAILABLE))
    }
    fn did_change(&self, _: &Path, _: &str, _: Revision) -> Result<(), LspError> {
        Err(LspError::new(UNAVAILABLE))
    }
    fn diagnostics(&self) -> Result<Vec<Diagnostic>, LspError> {
        Ok(Vec::new())
    }
    fn completion(
        &self,
        _: &Path,
        _: LspPosition,
        _: Revision,
    ) -> Result<Vec<CompletionItem>, LspError> {
        Err(LspError::new(UNAVAILABLE))
    }
    fn definition(
        &self,
        _: &Path,
        _: LspPosition,
        _: Revision,
    ) -> Result<Option<DefinitionTarget>, LspError> {
        Err(LspError::new(UNAVAILABLE))
    }
    fn shutdown(&self) -> Result<(), LspError> {
        Ok(())
    }
}

impl TerminalBackend for PrototypeBackend {
    fn start(&self, _: &Path, _: u16, _: u16) -> Result<TerminalId, TerminalError> {
        Err(TerminalError::new(UNAVAILABLE))
    }
    fn input(&self, _: &TerminalId, _: &[u8]) -> Result<(), TerminalError> {
        Err(TerminalError::new(UNAVAILABLE))
    }
    fn resize(&self, _: &TerminalId, _: u16, _: u16) -> Result<(), TerminalError> {
        Err(TerminalError::new(UNAVAILABLE))
    }
    fn poll(&self, _: &TerminalId) -> Result<Vec<TerminalEvent>, TerminalError> {
        Err(TerminalError::new(UNAVAILABLE))
    }
    fn terminate(&self, _: &TerminalId) -> Result<(), TerminalError> {
        Err(TerminalError::new(UNAVAILABLE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn save_checks_conflicts_and_keeps_changes_in_memory() {
        let backend = PrototypeBackend::new();
        let path = Path::new(ROOT).join("README.md");
        let initial = backend.read_file(&path).unwrap();
        backend
            .write_file(&path, b"edited", Some(&initial.fingerprint))
            .unwrap();
        assert_eq!(backend.read_file(&path).unwrap().bytes, b"edited");
        assert_eq!(
            backend
                .write_file(&path, b"stale", Some(&initial.fingerprint))
                .unwrap_err()
                .kind(),
            DocumentErrorKind::Conflict
        );
        assert!(
            backend
                .write_file(Path::new("/outside.md"), b"x", None)
                .is_err()
        );
    }
    #[test]
    fn tree_and_search_use_the_same_sample_files() {
        let backend = PrototypeBackend::new();
        let entries = backend.list_tree(Path::new(ROOT)).unwrap();
        assert!(
            entries
                .iter()
                .any(|entry| entry.relative_path == Path::new("src")
                    && entry.kind == FileEntryKind::Directory)
        );
        let results = backend
            .search(Path::new(ROOT), "Hello from", &AtomicBool::new(false))
            .unwrap();
        assert_eq!(results.len(), 1);
        assert!(backend.read_file(&results[0].path).is_ok());
        assert!(
            backend
                .search(Path::new(ROOT), "Hello", &AtomicBool::new(true))
                .unwrap()
                .is_empty()
        );
    }
}
