use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use document::{DocumentError, DocumentRepository, FileData, FileFingerprint};
use editor_core::ExecutionId;
use git::{FileDiff, GitBackend, GitError, GitErrorKind, RepositoryStatus, TaskWorktree};
pub use local_credentials::{CredentialStore, CredentialStoreError};
pub use local_git::LocalGitBackend;
pub use local_lsp::LocalLspBackend;
pub use local_search::LocalWorkspaceSearchBackend;
pub use local_tasks::{LocalTaskBackend, run_task_worker};
pub use local_terminal::{LocalTerminalBackend, default_profiles as default_terminal_profiles};
pub use local_workbench::LocalWorkbenchStore;
pub use local_workspace::LocalWorkspaceRepository;
pub use local_workspace_store::{WorkspaceRecord, WorkspaceStore, WorkspaceStoreError};
use lsp::{
    CompletionItem, DefinitionTarget, Diagnostic, LanguageServerBackend, LspError, LspPosition,
};
use task_runner::{TaskBackend, TaskControl, TaskError, TaskRecord};
use terminal::{
    TerminalBackend, TerminalDelta, TerminalError, TerminalEvent, TerminalId, TerminalInput,
};
use workspace::WorkspaceRepository;
use workspace::{FileEntry, SearchError, SearchHit, WorkspaceError, WorkspaceSearchBackend};

#[derive(Clone, Default)]
pub struct ConnectionGate(Arc<AtomicBool>);

impl ConnectionGate {
    pub fn connected() -> Self {
        Self(Arc::new(AtomicBool::new(true)))
    }

    pub fn connect(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn disconnect(&self) {
        self.0.store(false, Ordering::Release);
    }

    pub fn is_connected(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

pub struct LoopbackBackend<T> {
    inner: Arc<T>,
    gate: ConnectionGate,
}

impl<T> Clone for LoopbackBackend<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            gate: self.gate.clone(),
        }
    }
}

impl<T> LoopbackBackend<T> {
    pub fn new(inner: Arc<T>, gate: ConnectionGate) -> Self {
        Self { inner, gate }
    }

    pub fn gate(&self) -> &ConnectionGate {
        &self.gate
    }
}

impl<T: DocumentRepository> DocumentRepository for LoopbackBackend<T> {
    fn read_file(&self, path: &Path) -> Result<FileData, DocumentError> {
        if !self.gate.is_connected() {
            return Err(DocumentError::io("loopback workspace is disconnected"));
        }
        self.inner.read_file(path)
    }

    fn write_file(
        &self,
        path: &Path,
        content: &[u8],
        expected: Option<&FileFingerprint>,
    ) -> Result<FileFingerprint, DocumentError> {
        if !self.gate.is_connected() {
            return Err(DocumentError::io("loopback workspace is disconnected"));
        }
        self.inner.write_file(path, content, expected)
    }

    fn fingerprint(&self, path: &Path) -> Result<Option<FileFingerprint>, DocumentError> {
        if !self.gate.is_connected() {
            return Err(DocumentError::io("loopback workspace is disconnected"));
        }
        self.inner.fingerprint(path)
    }
}

impl<T: WorkspaceRepository> WorkspaceRepository for LoopbackBackend<T> {
    fn create_directory(&self, root: &Path, name: &str) -> Result<(), WorkspaceError> {
        if !self.gate.is_connected() {
            return Err(WorkspaceError::new("loopback workspace is disconnected"));
        }
        self.inner.create_directory(root, name)
    }
    fn list_tree(&self, root: &Path) -> Result<Vec<FileEntry>, WorkspaceError> {
        if !self.gate.is_connected() {
            return Err(WorkspaceError::new("loopback workspace is disconnected"));
        }
        self.inner.list_tree(root)
    }
}

impl<T: WorkspaceSearchBackend> WorkspaceSearchBackend for LoopbackBackend<T> {
    fn search(
        &self,
        root: &Path,
        query: &str,
        cancelled: &AtomicBool,
    ) -> Result<Vec<SearchHit>, SearchError> {
        if !self.gate.is_connected() {
            return Err(SearchError::new("loopback workspace is disconnected"));
        }
        self.inner.search(root, query, cancelled)
    }
}

impl<T: TaskBackend> TaskBackend for LoopbackBackend<T> {
    fn load(&self) -> Result<Vec<TaskRecord>, TaskError> {
        if !self.gate.is_connected() {
            return Err(TaskError::new("loopback workspace is disconnected"));
        }
        self.inner.load()
    }

    fn start(&self, record: &TaskRecord) -> Result<(), TaskError> {
        if !self.gate.is_connected() {
            return Err(TaskError::new("loopback workspace is disconnected"));
        }
        self.inner.start(record)
    }

    fn control(&self, execution_id: &ExecutionId, control: &TaskControl) -> Result<(), TaskError> {
        if !self.gate.is_connected() {
            return Err(TaskError::new("loopback workspace is disconnected"));
        }
        self.inner.control(execution_id, control)
    }
}

impl<T: GitBackend> GitBackend for LoopbackBackend<T> {
    fn watch_repository(
        &self,
        repository: &Path,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Box<dyn git::GitWatch>, GitError> {
        if !self.gate.is_connected() {
            return Err(GitError::new(
                GitErrorKind::Io,
                "loopback workspace is disconnected",
            ));
        }
        self.inner.watch_repository(repository, notify)
    }
    fn discover_repositories(&self, workspace: &Path) -> Result<Vec<PathBuf>, GitError> {
        if !self.gate.is_connected() {
            return Err(GitError::new(
                GitErrorKind::Io,
                "loopback workspace is disconnected",
            ));
        }
        self.inner.discover_repositories(workspace)
    }
    fn execute(
        &self,
        repository: &Path,
        request: git::GitActionRequest,
    ) -> Result<git::GitActionResult, GitError> {
        if !self.gate.is_connected() {
            return Err(GitError::new(
                GitErrorKind::Io,
                "loopback workspace is disconnected",
            ));
        }
        self.inner.execute(repository, request)
    }
    fn status(&self, repository: &Path) -> Result<RepositoryStatus, GitError> {
        if !self.gate.is_connected() {
            return Err(GitError::new(
                GitErrorKind::Io,
                "loopback workspace is disconnected",
            ));
        }
        self.inner.status(repository)
    }

    fn diff(&self, repository: &Path, path: &Path) -> Result<FileDiff, GitError> {
        if !self.gate.is_connected() {
            return Err(GitError::new(
                GitErrorKind::Io,
                "loopback workspace is disconnected",
            ));
        }
        self.inner.diff(repository, path)
    }

    fn create_worktree(
        &self,
        repository: &Path,
        task_id: &editor_core::TaskId,
    ) -> Result<TaskWorktree, GitError> {
        if !self.gate.is_connected() {
            return Err(GitError::new(
                GitErrorKind::Io,
                "loopback workspace is disconnected",
            ));
        }
        self.inner.create_worktree(repository, task_id)
    }

    fn worktrees(&self, repository: &Path) -> Result<Vec<TaskWorktree>, GitError> {
        if !self.gate.is_connected() {
            return Err(GitError::new(
                GitErrorKind::Io,
                "loopback workspace is disconnected",
            ));
        }
        self.inner.worktrees(repository)
    }

    fn import_file(&self, worktree: &TaskWorktree, path: &Path) -> Result<TaskWorktree, GitError> {
        if !self.gate.is_connected() {
            return Err(GitError::new(
                GitErrorKind::Io,
                "loopback workspace is disconnected",
            ));
        }
        self.inner.import_file(worktree, path)
    }

    fn discard_worktree(&self, worktree: &TaskWorktree) -> Result<TaskWorktree, GitError> {
        if !self.gate.is_connected() {
            return Err(GitError::new(
                GitErrorKind::Io,
                "loopback workspace is disconnected",
            ));
        }
        self.inner.discard_worktree(worktree)
    }
}

impl<T: TerminalBackend> TerminalBackend for LoopbackBackend<T> {
    fn start(&self, cwd: &Path, columns: u16, rows: u16) -> Result<TerminalId, TerminalError> {
        if !self.gate.is_connected() {
            return Err(TerminalError::new("loopback workspace is disconnected"));
        }
        self.inner.start(cwd, columns, rows)
    }

    fn start_profile(
        &self,
        cwd: &Path,
        columns: u16,
        rows: u16,
        profile: Option<&terminal::TerminalProfileId>,
    ) -> Result<TerminalId, TerminalError> {
        if !self.gate.is_connected() {
            return Err(TerminalError::new("loopback workspace is disconnected"));
        }
        self.inner.start_profile(cwd, columns, rows, profile)
    }

    fn profiles(&self, cwd: &Path) -> Result<terminal::TerminalProfiles, TerminalError> {
        if !self.gate.is_connected() {
            return Err(TerminalError::new("loopback workspace is disconnected"));
        }
        self.inner.profiles(cwd)
    }

    fn input(&self, id: &TerminalId, input: &TerminalInput) -> Result<(), TerminalError> {
        if !self.gate.is_connected() {
            return Err(TerminalError::new("loopback workspace is disconnected"));
        }
        self.inner.input(id, input)
    }

    fn resize(&self, id: &TerminalId, columns: u16, rows: u16) -> Result<(), TerminalError> {
        if !self.gate.is_connected() {
            return Err(TerminalError::new("loopback workspace is disconnected"));
        }
        self.inner.resize(id, columns, rows)
    }

    fn resize_delta(
        &self,
        id: &TerminalId,
        columns: u16,
        rows: u16,
    ) -> Result<TerminalDelta, TerminalError> {
        if !self.gate.is_connected() {
            return Err(TerminalError::new("loopback workspace is disconnected"));
        }
        self.inner.resize_delta(id, columns, rows)
    }

    fn poll(&self, id: &TerminalId) -> Result<Vec<TerminalEvent>, TerminalError> {
        if !self.gate.is_connected() {
            return Err(TerminalError::new("loopback workspace is disconnected"));
        }
        self.inner.poll(id)
    }

    fn terminate(&self, id: &TerminalId) -> Result<(), TerminalError> {
        if !self.gate.is_connected() {
            return Err(TerminalError::new("loopback workspace is disconnected"));
        }
        self.inner.terminate(id)
    }
}

impl<T: LanguageServerBackend> LanguageServerBackend for LoopbackBackend<T> {
    fn start(&self, workspace: &Path) -> Result<(), LspError> {
        if !self.gate.is_connected() {
            return Err(LspError::new("loopback workspace is disconnected"));
        }
        self.inner.start(workspace)
    }
    fn did_open(
        &self,
        path: &Path,
        text: &str,
        revision: document::Revision,
    ) -> Result<(), LspError> {
        if !self.gate.is_connected() {
            return Err(LspError::new("loopback workspace is disconnected"));
        }
        self.inner.did_open(path, text, revision)
    }
    fn did_change(
        &self,
        path: &Path,
        text: &str,
        revision: document::Revision,
    ) -> Result<(), LspError> {
        if !self.gate.is_connected() {
            return Err(LspError::new("loopback workspace is disconnected"));
        }
        self.inner.did_change(path, text, revision)
    }
    fn diagnostics(&self) -> Result<Vec<Diagnostic>, LspError> {
        if !self.gate.is_connected() {
            return Err(LspError::new("loopback workspace is disconnected"));
        }
        self.inner.diagnostics()
    }
    fn completion(
        &self,
        path: &Path,
        position: LspPosition,
        revision: document::Revision,
    ) -> Result<Vec<CompletionItem>, LspError> {
        if !self.gate.is_connected() {
            return Err(LspError::new("loopback workspace is disconnected"));
        }
        self.inner.completion(path, position, revision)
    }
    fn definition(
        &self,
        path: &Path,
        position: LspPosition,
        revision: document::Revision,
    ) -> Result<Option<DefinitionTarget>, LspError> {
        if !self.gate.is_connected() {
            return Err(LspError::new("loopback workspace is disconnected"));
        }
        self.inner.definition(path, position, revision)
    }
    fn shutdown(&self) -> Result<(), LspError> {
        if !self.gate.is_connected() {
            return Err(LspError::new("loopback workspace is disconnected"));
        }
        self.inner.shutdown()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_disconnects_and_reconnects_without_losing_workspace_data() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        std::fs::write(&path, "kept across reconnect\n").unwrap();
        let gate = ConnectionGate::connected();
        let backend = LoopbackBackend::new(Arc::new(LocalWorkspaceRepository), gate.clone());

        assert!(backend.read_file(&path).is_ok());
        gate.disconnect();
        assert!(backend.read_file(&path).is_err());
        assert!(backend.list_tree(directory.path()).is_err());
        gate.connect();

        let restored = backend.read_file(&path).unwrap();
        assert_eq!(restored.bytes, b"kept across reconnect\n");
        assert!(backend.list_tree(directory.path()).is_ok());
    }
}
