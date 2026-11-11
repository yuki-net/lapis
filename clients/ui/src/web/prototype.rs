//! UI検証用のメモリーWorkspace。OS・外部プロセスには接続しない。

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use desktop_client::{
    ConversationRecord, ConversationRepository, ConversationState, WorkspaceDialog,
    WorkspaceStateRepository, WorkspaceViewState,
};
use document::{
    Document, DocumentCache, DocumentConnection, DocumentError, FileData, FileFingerprint, Revision,
};
use editor_core::{ConversationId, DocumentId, ExecutionId, TaskId, WorkspaceId};
use git::{GitConnection, GitDiffSnapshot, GitError, GitErrorKind, GitSnapshot, WorktreeSnapshot};
use lsp::{
    CompletionItem, DefinitionTarget, Diagnostic, LanguageServerBackend, LspError, LspPosition,
};
use settings::{SettingsDocument, SettingsError, SettingsRepository, SettingsScope};
use task::{TaskConnection, TaskControl, TaskError, TaskSnapshot, TaskStart};
use terminal::{TerminalBackend, TerminalError, TerminalEvent, TerminalId};
use workspace::{
    FileEntry, FileEntryKind, SearchError, SearchHit, WorkspaceError, WorkspaceRepository,
    WorkspaceSearchBackend,
};

pub(super) const ROOT: &str = "/Lapis Web";
const UNAVAILABLE: &str = "Webではこの機能に接続していません";

#[derive(Clone)]
pub(super) struct PrototypeBackend {
    files: Arc<Mutex<BTreeMap<PathBuf, Vec<u8>>>>,
    documents: Arc<Mutex<HashMap<DocumentId, Document>>>,
    snapshot: Arc<Mutex<Option<WorkspaceViewState>>>,
    settings: Arc<Mutex<SettingsDocument>>,
    conversations: Arc<Mutex<ConversationState>>,
    next_document: Arc<Mutex<u64>>,
}

impl PrototypeBackend {
    pub(super) fn new() -> Self {
        let backend = Self {
            files: Arc::new(Mutex::new(BTreeMap::new())),
            documents: Arc::new(Mutex::new(HashMap::new())),
            snapshot: Arc::new(Mutex::new(None)),
            settings: Arc::new(Mutex::new(SettingsDocument::empty(SettingsScope::User))),
            conversations: Arc::new(Mutex::new(ConversationState::default())),
            next_document: Arc::new(Mutex::new(1)),
        };
        for (name, content) in [
            (
                "README.md",
                "# Lapis Web\n\nDesktopと同じRust/GPUIの画面をブラウザーで動かしています。\n\n## 試せること\n\n- テキスト編集、選択、Undo/Redo\n- ファイルツリーから文書を開く、タブ切替\n- Ctrl+Shift+K: コマンドパレット\n- Ctrl+F: エディター内検索\n- Ctrl+J: 下部パネルの切替\n- Markdownプレビューとテーマ変更\n- パネルとウィンドウのリサイズ\n\n## 範囲\n\nファイルと設定はメモリー上だけにあります。ページを再読み込みすると元に戻ります。\nTerminal、Git、LSP、外部runnerはまだ接続していません。\n",
            ),
            (
                "src/main.rs",
                "// ブラウザーで動くDesktopエディター。\n// このサンプルを編集してみてください。変更はメモリー上に残ります。\n\nfn main() {\n    let platform = \"GPUI Web\";\n    println!(\"Hello from {platform}!\");\n}\n",
            ),
            (
                "notes.md",
                "# Notes\n\nタブとMarkdownプレビューの確認用です。\n\n- [ ] キーボード入力を試す\n- [ ] パネルのリサイズを試す\n- [ ] テーマを変えてみる\n",
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

    fn cache(&self, id: &DocumentId, document: &Document) -> DocumentCache {
        let actual = document.path().and_then(|path| {
            self.files
                .lock()
                .unwrap()
                .get(path)
                .map(|bytes| fingerprint(bytes))
        });
        DocumentCache {
            reused: true,
            id: id.clone(),
            document: document.clone(),
            can_undo: document.can_undo(),
            can_redo: document.can_redo(),
            external_change: document.external_change(actual.as_ref()),
            owner: None,
            is_owner: false,
        }
    }

    fn mutate(
        &self,
        id: &DocumentId,
        revision: Revision,
        operation: impl FnOnce(&mut Document) -> Result<(), DocumentError>,
    ) -> Result<DocumentCache, DocumentError> {
        let mut documents = self.documents.lock().unwrap();
        let document = documents
            .get_mut(id)
            .ok_or_else(|| DocumentError::io("Documentが見つかりません"))?;
        if document.revision() != revision {
            return Err(DocumentError::conflict(
                "Documentのrevisionが競合しています",
            ));
        }
        operation(document)?;
        Ok(self.cache(id, document))
    }
}

fn fingerprint(bytes: &[u8]) -> FileFingerprint {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    FileFingerprint::new(bytes.len() as u64, None, hasher.finish())
}

impl DocumentConnection for PrototypeBackend {
    fn fork(&self) -> Arc<dyn DocumentConnection> {
        Arc::new(self.clone())
    }

    fn workspace(&self, _: &Path) -> Result<WorkspaceId, DocumentError> {
        Ok(WorkspaceId::new("lapis-web"))
    }

    fn create(&self, _: Option<&Path>) -> Result<DocumentCache, DocumentError> {
        let mut next = self.next_document.lock().unwrap();
        let id = DocumentId::new(format!("web-document-{next}"));
        *next += 1;
        let document = Document::new();
        let mut cache = self.cache(&id, &document);
        cache.reused = false;
        self.documents.lock().unwrap().insert(id, document);
        Ok(cache)
    }

    fn open(&self, path: &Path) -> Result<DocumentCache, DocumentError> {
        let mut documents = self.documents.lock().unwrap();
        if let Some((id, document)) = documents
            .iter()
            .find(|(_, document)| document.path() == Some(path))
        {
            return Ok(self.cache(id, document));
        }
        let bytes = {
            let files = self.files.lock().unwrap();
            files
                .get(path)
                .cloned()
                .ok_or_else(|| DocumentError::io("サンプルファイルが見つかりません"))?
        };
        let document = Document::from_file(
            path.to_owned(),
            FileData {
                fingerprint: fingerprint(&bytes),
                bytes,
            },
        )?;
        let mut next = self.next_document.lock().unwrap();
        let id = DocumentId::new(format!("web-document-{next}"));
        *next += 1;
        let mut cache = self.cache(&id, &document);
        cache.reused = false;
        documents.insert(id, document);
        Ok(cache)
    }

    fn attach(&self, id: &DocumentId) -> Result<DocumentCache, DocumentError> {
        let documents = self.documents.lock().unwrap();
        Ok(self.cache(
            id,
            documents
                .get(id)
                .ok_or_else(|| DocumentError::io("Documentが見つかりません"))?,
        ))
    }

    fn edit(
        &self,
        id: &DocumentId,
        revision: Revision,
        range: Range<usize>,
        replacement: &str,
    ) -> Result<DocumentCache, DocumentError> {
        self.mutate(id, revision, |document| {
            document.replace_char_range(range, replacement).map(|_| ())
        })
    }

    fn history(
        &self,
        id: &DocumentId,
        revision: Revision,
        undo: bool,
    ) -> Result<DocumentCache, DocumentError> {
        self.mutate(id, revision, |document| {
            if undo {
                document.undo();
            } else {
                document.redo();
            }
            Ok(())
        })
    }

    fn save(
        &self,
        id: &DocumentId,
        revision: Revision,
        path: Option<PathBuf>,
    ) -> Result<DocumentCache, DocumentError> {
        self.mutate(id, revision, |document| {
            let path = path
                .or_else(|| document.path().map(Path::to_owned))
                .ok_or_else(|| DocumentError::io("保存先がありません"))?;
            let file = FileData {
                bytes: document.encoded_bytes(),
                fingerprint: fingerprint(&document.encoded_bytes()),
            };
            let mut files = self.files.lock().unwrap();
            files.insert(path.clone(), file.bytes.clone());
            document.mark_saved(path, file.fingerprint);
            Ok(())
        })
    }

    fn reload(&self, id: &DocumentId, revision: Revision) -> Result<DocumentCache, DocumentError> {
        self.mutate(id, revision, |document| {
            if document.is_dirty() {
                return Err(DocumentError::conflict("未保存の変更があります"));
            }
            let path = document
                .path()
                .ok_or_else(|| DocumentError::io("保存先がありません"))?;
            let files = self.files.lock().unwrap();
            let bytes = files
                .get(path)
                .ok_or_else(|| DocumentError::io("ファイルが見つかりません"))?;
            document.reload(FileData {
                bytes: bytes.clone(),
                fingerprint: fingerprint(bytes),
            })
        })
    }

    fn discard(&self, id: &DocumentId, revision: Revision) -> Result<DocumentCache, DocumentError> {
        self.mutate(id, revision, |document| {
            let saved = if document.saved_fingerprint().is_some() {
                let path = document
                    .path()
                    .ok_or_else(|| DocumentError::conflict("保存先がありません"))?;
                let files = self.files.lock().unwrap();
                let bytes = files
                    .get(path)
                    .ok_or_else(|| DocumentError::conflict("保存済みの内容を読み込めません"))?;
                Some(FileData {
                    bytes: bytes.clone(),
                    fingerprint: fingerprint(bytes),
                })
            } else {
                None
            };
            document.discard(saved)
        })
    }

    fn close(&self, id: &DocumentId) -> Result<(), DocumentError> {
        self.documents.lock().unwrap().remove(id);
        Ok(())
    }
}

impl WorkspaceRepository for PrototypeBackend {
    fn create_directory(&self, root: &Path, name: &str) -> Result<(), WorkspaceError> {
        if root != Path::new(ROOT) {
            return Err(WorkspaceError::new("サンプルWorkspaceを開いてください"));
        }
        let directory = root.join(name);
        self.files.lock().unwrap().insert(directory, Vec::new());
        Ok(())
    }

    fn list_tree(&self, root: &Path) -> Result<Vec<FileEntry>, WorkspaceError> {
        if root != Path::new(ROOT) {
            return Err(WorkspaceError::new("サンプルWorkspaceを開いてください"));
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
    fn load(&self) -> Result<Option<WorkspaceViewState>, WorkspaceError> {
        Ok(self.snapshot.lock().unwrap().clone())
    }
    fn save(&self, snapshot: &WorkspaceViewState) -> Result<(), WorkspaceError> {
        *self.snapshot.lock().unwrap() = Some(snapshot.clone());
        Ok(())
    }
}

impl SettingsRepository for PrototypeBackend {
    fn load(&self) -> Result<SettingsDocument, SettingsError> {
        Ok(self.settings.lock().unwrap().clone())
    }
    fn save(&self, settings: &SettingsDocument) -> Result<(), SettingsError> {
        *self.settings.lock().unwrap() = settings.clone();
        Ok(())
    }
}

impl ConversationRepository for PrototypeBackend {
    fn load(&self) -> Result<ConversationState, WorkspaceError> {
        Ok(self.conversations.lock().unwrap().clone())
    }
    fn save(
        &self,
        _: &ConversationState,
        projects: &[desktop_client::Project],
        records: &[ConversationRecord],
        active: &ConversationId,
    ) -> Result<(), WorkspaceError> {
        *self.conversations.lock().unwrap() =
            (projects.to_vec(), records.to_vec(), Some(active.clone()));
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

impl TaskConnection for PrototypeBackend {
    fn load(&self, _: &WorkspaceId) -> Result<Vec<TaskSnapshot>, TaskError> {
        Ok(Vec::new())
    }
    fn start(&self, _: &WorkspaceId, _: TaskStart) -> Result<TaskSnapshot, TaskError> {
        Err(TaskError::new(UNAVAILABLE))
    }
    fn control(&self, _: &WorkspaceId, _: &ExecutionId, _: &TaskControl) -> Result<(), TaskError> {
        Err(TaskError::new(UNAVAILABLE))
    }
}

impl GitConnection for PrototypeBackend {
    fn status(&self, _: &WorkspaceId, _: Option<&TaskId>) -> Result<GitSnapshot, GitError> {
        Err(GitError::new(GitErrorKind::Io, UNAVAILABLE))
    }
    fn diff(
        &self,
        _: &WorkspaceId,
        _: Option<&TaskId>,
        _: &str,
    ) -> Result<GitDiffSnapshot, GitError> {
        Err(GitError::new(GitErrorKind::Io, UNAVAILABLE))
    }
    fn worktrees(&self, _: &WorkspaceId) -> Result<Vec<WorktreeSnapshot>, GitError> {
        Ok(Vec::new())
    }
    fn create_worktree(&self, _: &WorkspaceId) -> Result<WorktreeSnapshot, GitError> {
        Err(GitError::new(GitErrorKind::Io, UNAVAILABLE))
    }
    fn import_file(
        &self,
        _: &WorkspaceId,
        _: &TaskId,
        _: &str,
    ) -> Result<WorktreeSnapshot, GitError> {
        Err(GitError::new(GitErrorKind::Io, UNAVAILABLE))
    }
    fn discard_worktree(&self, _: &WorkspaceId, _: &TaskId) -> Result<WorktreeSnapshot, GitError> {
        Err(GitError::new(GitErrorKind::Io, UNAVAILABLE))
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
        Ok(None)
    }
    fn shutdown(&self) -> Result<(), LspError> {
        Ok(())
    }
}

impl TerminalBackend for PrototypeBackend {
    fn start(&self, _: &Path, _: u16, _: u16) -> Result<TerminalId, TerminalError> {
        Err(TerminalError::new(UNAVAILABLE))
    }
    fn input(&self, _: &TerminalId, _: &terminal::TerminalInput) -> Result<(), TerminalError> {
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
    use document::ExternalChange;

    #[test]
    fn sample_files_are_listed_and_searchable() {
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
        assert!(backend.open(&results[0].path).is_ok());
    }

    #[test]
    fn edits_stay_in_memory_and_survive_save() {
        let backend = PrototypeBackend::new();
        let opened = backend.open(&Path::new(ROOT).join("README.md")).unwrap();
        let revision = opened.document.revision();
        let edited = backend.edit(&opened.id, revision, 0..0, "prefix ").unwrap();
        assert!(edited.document.content_for_save().starts_with("prefix "));
        assert!(edited.document.is_dirty());
        backend
            .save(&opened.id, edited.document.revision(), None)
            .unwrap();
        backend.close(&opened.id).unwrap();
        let reopened = backend.open(&Path::new(ROOT).join("README.md")).unwrap();
        assert!(reopened.document.content_for_save().starts_with("prefix "));
    }

    #[test]
    fn external_change_is_reported_when_memory_file_changes() {
        let backend = PrototypeBackend::new();
        let path = Path::new(ROOT).join("notes.md");
        let opened = backend.open(&path).unwrap();
        backend
            .files
            .lock()
            .unwrap()
            .insert(path.clone(), b"changed".to_vec());
        let inspected = backend.attach(&opened.id).unwrap();
        assert_eq!(inspected.external_change, ExternalChange::Modified);
    }
}
