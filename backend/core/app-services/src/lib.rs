use std::{
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

pub use lapis_document::Encoding;
use lapis_document::{
    Document, DocumentError, DocumentRepository, ExternalChange, Position, Revision,
};
use lapis_editor_core::{ConversationId, DocumentId, ExecutionId, TaskId, WorkspaceId};
use lapis_git::{FileDiff, GitBackend, GitError, RepositoryStatus, TaskWorktree};
use lapis_localization::LocaleId;
use lapis_lsp::{
    CompletionItem, DefinitionTarget, Diagnostic, LanguageServerBackend, LspError, LspPosition,
};
use lapis_settings::{GlobalSettings, GlobalSettingsRepository, SettingsError};
use lapis_task_runner::{
    Execution, ExecutionStatus, Task, TaskBackend, TaskControl, TaskError, TaskMode, TaskRecord,
    unix_time_ms,
};
use lapis_terminal::{
    TerminalBackend, TerminalError, TerminalEvent, TerminalId, TerminalSnapshot, TerminalStatus,
};
use lapis_workspace::{
    DocumentViewState, FileEntry, SearchError, SearchHit, WorkspaceError, WorkspaceSearchBackend,
    WorkspaceSnapshot, WorkspaceStateRepository,
};
use serde::{Deserialize, Serialize};

/// グローバル設定を UI に公開し、変更を永続化するアプリケーション境界。
#[derive(Clone)]
pub struct SettingsSession {
    repository: Arc<dyn GlobalSettingsRepository>,
    settings: Arc<std::sync::Mutex<GlobalSettings>>,
}

impl SettingsSession {
    pub fn load(repository: Arc<dyn GlobalSettingsRepository>) -> Result<Self, SettingsError> {
        let settings = repository.load()?;
        Ok(Self {
            repository,
            settings: Arc::new(std::sync::Mutex::new(settings)),
        })
    }

    pub fn settings(&self) -> GlobalSettings {
        self.settings
            .lock()
            .expect("settings state lock failed")
            .clone()
    }

    pub fn set_locale(&self, locale: LocaleId) -> Result<(), SettingsError> {
        let mut settings = self.settings.lock().expect("settings state lock failed");
        if settings.locale == locale {
            return Ok(());
        }
        let previous = settings.clone();
        settings.locale = locale;
        if let Err(error) = self.repository.save(&settings) {
            *settings = previous;
            return Err(error);
        }
        Ok(())
    }

    pub fn set_theme(&self, theme: String) -> Result<(), SettingsError> {
        let mut settings = self.settings.lock().expect("settings state lock failed");
        if settings.theme == theme {
            return Ok(());
        }
        let previous = settings.clone();
        settings.theme = theme;
        if let Err(error) = self.repository.save(&settings) {
            *settings = previous;
            return Err(error);
        }
        Ok(())
    }
}

/// ネイティブダイアログを利用するユースケース側の契約。
pub trait WorkspaceDialog: Send + Sync {
    fn choose_workspace_path(&self) -> Option<PathBuf>;
    fn choose_file_path(&self) -> Option<PathBuf> {
        None
    }
    fn choose_save_path(&self, suggested_name: &str) -> Option<PathBuf>;
}

/// Workspaceの列挙とDocument I/Oをまとめるbackend契約。
pub trait WorkspaceRepository: DocumentRepository {
    fn list_tree(&self, root: &Path) -> Result<Vec<FileEntry>, WorkspaceError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentAction {
    Completed,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentCloseDisposition {
    PreserveChanges,
    DiscardChanges,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConversationViewState {
    /// 新しいレイアウト形式。位置ごとのタブ・選択状態・サイズを一律で保持する。
    pub panels: Vec<PanelViewState>,
    // 旧セッションとの互換読み込み用。次回保存時は panels が正とする。
    pub active_tool: String,
    pub side_panel: Option<String>,
    pub bottom_panel: Option<String>,
    pub tool_width: f32,
    pub side_width: f32,
    pub bottom_height: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PanelViewState {
    pub position: String,
    pub tabs: Vec<String>,
    pub active_tab: Option<String>,
    pub open: bool,
    pub size: f32,
}

impl Default for ConversationViewState {
    fn default() -> Self {
        Self {
            panels: Vec::new(),
            active_tool: "files".to_owned(),
            side_panel: None,
            bottom_panel: None,
            tool_width: 320.0,
            side_width: 340.0,
            bottom_height: 220.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoredTerminal {
    pub cwd: PathBuf,
    pub status: TerminalStatus,
    pub columns: u16,
    pub rows: u16,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConversationRecord {
    pub id: ConversationId,
    pub title: String,
    pub workspace: WorkspaceSnapshot,
    pub view: ConversationViewState,
    pub selected_execution: Option<ExecutionId>,
    pub terminals: Vec<RestoredTerminal>,
}

pub trait ConversationRepository: Send + Sync {
    fn load(&self) -> Result<(Vec<ConversationRecord>, Option<ConversationId>), WorkspaceError>;
    fn save(
        &self,
        records: &[ConversationRecord],
        active: &ConversationId,
    ) -> Result<(), WorkspaceError>;
}

pub struct ConversationSession {
    repository: Arc<dyn ConversationRepository>,
    records: Vec<ConversationRecord>,
    active: ConversationId,
    next_id: u64,
}

impl ConversationSession {
    pub fn new(repository: Arc<dyn ConversationRepository>, initial: WorkspaceSnapshot) -> Self {
        let loaded = repository.load().ok();
        let (mut records, active) = loaded.unwrap_or_default();
        if records.is_empty() {
            records.push(ConversationRecord {
                id: ConversationId::new("conversation-1"),
                title: "Conversation 1".to_owned(),
                workspace: initial,
                view: ConversationViewState::default(),
                selected_execution: None,
                terminals: Vec::new(),
            });
        }
        let active = active
            .filter(|id| records.iter().any(|record| &record.id == id))
            .unwrap_or_else(|| records[0].id.clone());
        let next_id = records.len() as u64 + 1;
        Self {
            repository,
            records,
            active,
            next_id,
        }
    }

    pub fn records(&self) -> &[ConversationRecord] {
        &self.records
    }
    pub fn active_id(&self) -> &ConversationId {
        &self.active
    }
    pub fn active_record(&self) -> Option<&ConversationRecord> {
        self.records.iter().find(|record| record.id == self.active)
    }

    pub fn active_view(&self) -> ConversationViewState {
        self.records
            .iter()
            .find(|record| record.id == self.active)
            .map(|record| record.view.clone())
            .unwrap_or_default()
    }

    pub fn restore_active(
        &self,
        editor: &mut EditorSession,
    ) -> Result<ConversationViewState, WorkspaceError> {
        let record = self
            .records
            .iter()
            .find(|record| record.id == self.active)
            .ok_or_else(|| WorkspaceError::new("Conversation が見つかりません"))?;
        editor.restore_snapshot(record.workspace.clone())?;
        Ok(record.view.clone())
    }

    /// 開いた Project が最後に保存した Conversation と同じ場合だけ、その状態を復元する。
    /// アプリ起動直後に無条件で以前の Project を開かないための境界でもある。
    pub fn restore_matching_workspace(
        &self,
        editor: &mut EditorSession,
    ) -> Result<Option<ConversationViewState>, WorkspaceError> {
        let Some(current_root) = editor.workspace_root() else {
            return Ok(None);
        };
        let Some(record) = self.active_record() else {
            return Ok(None);
        };
        if record.workspace.root.as_deref() != Some(current_root) {
            return Ok(None);
        }
        editor.restore_snapshot(record.workspace.clone())?;
        Ok(Some(record.view.clone()))
    }

    pub fn capture(
        &mut self,
        editor: &EditorSession,
        view: ConversationViewState,
        selected_execution: Option<ExecutionId>,
        terminals: &[TerminalSnapshot],
    ) -> Result<(), WorkspaceError> {
        if let Some(record) = self
            .records
            .iter_mut()
            .find(|record| record.id == self.active)
        {
            record.workspace = editor.snapshot();
            record.view = view;
            record.selected_execution = selected_execution;
            record.terminals = terminals
                .iter()
                .map(|terminal| RestoredTerminal {
                    cwd: terminal.cwd.clone(),
                    status: terminal.status,
                    columns: terminal.columns,
                    rows: terminal.rows,
                })
                .collect();
        }
        self.repository.save(&self.records, &self.active)
    }

    pub fn create(
        &mut self,
        editor: &EditorSession,
        view: ConversationViewState,
    ) -> Result<ConversationId, WorkspaceError> {
        let id = ConversationId::new(format!("conversation-{}", self.next_id));
        self.next_id += 1;
        self.records.push(ConversationRecord {
            id: id.clone(),
            title: format!("Conversation {}", self.records.len() + 1),
            workspace: editor.snapshot(),
            view,
            selected_execution: None,
            terminals: Vec::new(),
        });
        self.active = id.clone();
        self.repository.save(&self.records, &self.active)?;
        Ok(id)
    }

    pub fn switch(
        &mut self,
        id: &ConversationId,
        editor: &mut EditorSession,
    ) -> Result<ConversationViewState, WorkspaceError> {
        let record = self
            .records
            .iter()
            .find(|record| &record.id == id)
            .cloned()
            .ok_or_else(|| WorkspaceError::new("Conversation が見つかりません"))?;
        editor.restore_snapshot(record.workspace)?;
        self.active = id.clone();
        self.repository.save(&self.records, &self.active)?;
        Ok(record.view)
    }

    pub fn repository(&self) -> Arc<dyn ConversationRepository> {
        self.repository.clone()
    }
}

pub struct GitSession {
    backend: Arc<dyn GitBackend>,
    repository_root: Option<PathBuf>,
    status: Option<RepositoryStatus>,
    worktrees: Vec<TaskWorktree>,
    worktree_statuses: Vec<(TaskId, RepositoryStatus)>,
    selected_diff: Option<FileDiff>,
}

impl GitSession {
    pub fn new(backend: Arc<dyn GitBackend>) -> Self {
        Self {
            backend,
            repository_root: None,
            status: None,
            worktrees: Vec::new(),
            worktree_statuses: Vec::new(),
            selected_diff: None,
        }
    }

    pub fn status(&self) -> Option<&RepositoryStatus> {
        self.status.as_ref()
    }

    pub fn worktrees(&self) -> &[TaskWorktree] {
        &self.worktrees
    }

    pub fn selected_diff(&self) -> Option<&FileDiff> {
        self.selected_diff.as_ref()
    }

    pub fn worktree_statuses(&self) -> &[(TaskId, RepositoryStatus)] {
        &self.worktree_statuses
    }

    pub fn refresh(&mut self, repository: &Path) -> Result<bool, GitError> {
        let status = self.backend.status(repository)?;
        let worktrees = self.backend.worktrees(repository)?;
        let worktree_statuses = worktrees
            .iter()
            .filter(|worktree| worktree.path.is_dir())
            .filter_map(|worktree| {
                self.backend
                    .status(&worktree.path)
                    .ok()
                    .map(|status| (worktree.task_id.clone(), status))
            })
            .collect::<Vec<_>>();
        let changed = self.status.as_ref() != Some(&status)
            || self.worktrees != worktrees
            || self.worktree_statuses != worktree_statuses;
        self.repository_root = Some(status.root.clone());
        self.status = Some(status);
        self.worktrees = worktrees;
        self.worktree_statuses = worktree_statuses;
        Ok(changed)
    }

    pub fn select_diff(&mut self, path: &Path) -> Result<(), GitError> {
        let root = self.repository_root.as_deref().ok_or_else(|| {
            GitError::new(
                lapis_git::GitErrorKind::NotRepository,
                "Git repository がありません",
            )
        })?;
        self.selected_diff = Some(self.backend.diff(root, path)?);
        Ok(())
    }

    pub fn select_worktree_diff(&mut self, task_id: &TaskId, path: &Path) -> Result<(), GitError> {
        let worktree = self
            .worktrees
            .iter()
            .find(|worktree| &worktree.task_id == task_id)
            .ok_or_else(|| {
                GitError::new(lapis_git::GitErrorKind::Io, "Task worktree がありません")
            })?;
        self.selected_diff = Some(self.backend.diff(&worktree.path, path)?);
        Ok(())
    }

    pub fn create_task_worktree(
        &mut self,
        repository: &Path,
        task_id: &TaskId,
    ) -> Result<TaskWorktree, GitError> {
        let worktree = self.backend.create_worktree(repository, task_id)?;
        self.worktrees.push(worktree.clone());
        Ok(worktree)
    }

    pub fn import_file(&mut self, task_id: &TaskId, path: &Path) -> Result<(), GitError> {
        let index = self
            .worktrees
            .iter()
            .position(|worktree| &worktree.task_id == task_id)
            .ok_or_else(|| {
                GitError::new(lapis_git::GitErrorKind::Io, "Task worktree がありません")
            })?;
        self.worktrees[index] = self.backend.import_file(&self.worktrees[index], path)?;
        if let Some(root) = self.repository_root.clone() {
            let _ = self.refresh(&root)?;
        }
        Ok(())
    }

    pub fn discard_worktree(&mut self, task_id: &TaskId) -> Result<(), GitError> {
        let index = self
            .worktrees
            .iter()
            .position(|worktree| &worktree.task_id == task_id)
            .ok_or_else(|| {
                GitError::new(lapis_git::GitErrorKind::Io, "Task worktree がありません")
            })?;
        self.worktrees[index] = self.backend.discard_worktree(&self.worktrees[index])?;
        Ok(())
    }

    pub fn backend(&self) -> Arc<dyn GitBackend> {
        self.backend.clone()
    }
}

pub struct TerminalSession {
    backend: Arc<dyn TerminalBackend>,
    terminals: Vec<TerminalSnapshot>,
}

impl TerminalSession {
    pub fn new(backend: Arc<dyn TerminalBackend>) -> Self {
        Self {
            backend,
            terminals: Vec::new(),
        }
    }

    pub fn terminals(&self) -> &[TerminalSnapshot] {
        &self.terminals
    }

    /// 前回終了時の端末一覧を、再実行せず参照専用の要約として復元する。
    pub fn restore_summaries(&mut self, terminals: &[RestoredTerminal]) {
        self.terminals = terminals
            .iter()
            .enumerate()
            .map(|(index, terminal)| TerminalSnapshot {
                id: TerminalId::new(format!("restored-terminal-{}", index + 1)),
                cwd: terminal.cwd.clone(),
                status: TerminalStatus::Exited,
                columns: terminal.columns,
                rows: terminal.rows,
                output: Vec::new(),
                output_sequence: 0,
                output_truncated: false,
            })
            .collect();
    }

    pub fn terminate_all(&mut self) -> Result<(), TerminalError> {
        let running = self
            .terminals
            .iter()
            .filter(|terminal| terminal.status == TerminalStatus::Running)
            .map(|terminal| terminal.id.clone())
            .collect::<Vec<_>>();
        for id in running {
            self.terminate(&id)?;
        }
        Ok(())
    }

    pub fn start(
        &mut self,
        cwd: &Path,
        columns: u16,
        rows: u16,
    ) -> Result<TerminalId, TerminalError> {
        let id = self.backend.start(cwd, columns, rows)?;
        self.terminals.push(TerminalSnapshot {
            id: id.clone(),
            cwd: cwd.to_owned(),
            status: TerminalStatus::Running,
            columns,
            rows,
            output: Vec::new(),
            output_sequence: 0,
            output_truncated: false,
        });
        Ok(id)
    }

    pub fn input(&self, id: &TerminalId, text: &str) -> Result<(), TerminalError> {
        self.input_bytes(id, text.as_bytes())
    }

    pub fn input_bytes(&self, id: &TerminalId, bytes: &[u8]) -> Result<(), TerminalError> {
        self.backend.input(id, bytes)
    }

    pub fn resize(
        &mut self,
        id: &TerminalId,
        columns: u16,
        rows: u16,
    ) -> Result<(), TerminalError> {
        self.backend.resize(id, columns, rows)?;
        if let Some(terminal) = self
            .terminals
            .iter_mut()
            .find(|terminal| terminal.id == *id)
        {
            terminal.columns = columns;
            terminal.rows = rows;
        }
        Ok(())
    }

    pub fn refresh(&mut self) -> Result<bool, TerminalError> {
        let mut changed = false;
        for terminal in &mut self.terminals {
            if terminal.status != TerminalStatus::Running {
                continue;
            }
            for event in self.backend.poll(&terminal.id)? {
                match event {
                    TerminalEvent::Output(output) => {
                        if output.sequence <= terminal.output_sequence {
                            continue;
                        }
                        terminal.output.extend(output.data);
                        terminal.output_sequence = output.sequence;
                        const OUTPUT_LIMIT: usize = 256 * 1024;
                        if terminal.output.len() > OUTPUT_LIMIT {
                            let start = terminal.output.len() - OUTPUT_LIMIT;
                            terminal.output.drain(..start);
                            terminal.output_truncated = true;
                        }
                    }
                    TerminalEvent::Exited { .. } => terminal.status = TerminalStatus::Exited,
                    TerminalEvent::Failed(_) => {
                        terminal.status = TerminalStatus::Failed;
                    }
                }
                changed = true;
            }
        }
        Ok(changed)
    }

    pub fn terminate(&mut self, id: &TerminalId) -> Result<(), TerminalError> {
        self.backend.terminate(id)?;
        if let Some(terminal) = self
            .terminals
            .iter_mut()
            .find(|terminal| terminal.id == *id)
        {
            terminal.status = TerminalStatus::Exited;
        }
        Ok(())
    }

    pub fn backend(&self) -> Arc<dyn TerminalBackend> {
        self.backend.clone()
    }
}

pub struct LspSession {
    backend: Arc<dyn LanguageServerBackend>,
    attempted_workspace: Option<PathBuf>,
    started_workspace: Option<PathBuf>,
    synced_document: Option<(PathBuf, Revision)>,
    diagnostics: Vec<Diagnostic>,
    diagnostics_receiver: Option<mpsc::Receiver<Result<Vec<Diagnostic>, LspError>>>,
    last_error: Option<String>,
}

impl LspSession {
    pub fn new(backend: Arc<dyn LanguageServerBackend>) -> Self {
        Self {
            backend,
            attempted_workspace: None,
            started_workspace: None,
            synced_document: None,
            diagnostics: Vec::new(),
            diagnostics_receiver: None,
            last_error: None,
        }
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn is_started(&self) -> bool {
        self.started_workspace.is_some()
    }

    pub fn sync_active(&mut self, editor: &EditorSession) -> Result<bool, LspError> {
        let Some(workspace) = editor.workspace_root() else {
            return Ok(false);
        };
        let Some(path) = editor.active_path() else {
            return Ok(false);
        };
        if path.extension().and_then(|value| value.to_str()) != Some("rs") {
            return Ok(false);
        }
        if self.started_workspace.as_deref() != Some(workspace) {
            if self.attempted_workspace.as_deref() == Some(workspace) && self.last_error.is_some() {
                return Ok(false);
            }
            if self.started_workspace.is_some() {
                self.backend.shutdown()?;
            }
            self.attempted_workspace = Some(workspace.to_owned());
            if let Err(error) = self.backend.start(workspace) {
                self.last_error = Some(error.to_string());
                return Err(error);
            }
            self.started_workspace = Some(workspace.to_owned());
            self.synced_document = None;
            self.last_error = None;
        }
        let revision = editor.active_revision();
        let current = (path.to_owned(), revision);
        if self.synced_document.as_ref() == Some(&current) {
            return Ok(false);
        }
        let text = editor
            .active_text()
            .map_err(|error| LspError::new(error.to_string()))?;
        if self
            .synced_document
            .as_ref()
            .is_some_and(|(previous, _)| previous == path)
        {
            self.backend.did_change(path, &text, revision)?;
        } else {
            self.backend.did_open(path, &text, revision)?;
        }
        self.synced_document = Some(current);
        Ok(true)
    }

    pub fn refresh(&mut self) -> Result<bool, LspError> {
        if self.started_workspace.is_none() {
            return Ok(false);
        }
        if let Some(receiver) = &self.diagnostics_receiver {
            match receiver.try_recv() {
                Ok(result) => {
                    self.diagnostics_receiver = None;
                    let diagnostics = result?;
                    let changed = diagnostics != self.diagnostics;
                    self.diagnostics = diagnostics;
                    return Ok(changed);
                }
                Err(mpsc::TryRecvError::Empty) => return Ok(false),
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.diagnostics_receiver = None;
                    return Err(LspError::new("LSP diagnostics worker disconnected"));
                }
            }
        }
        let backend = self.backend.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(backend.diagnostics());
        });
        self.diagnostics_receiver = Some(receiver);
        Ok(false)
    }

    pub fn completion(
        &self,
        editor: &EditorSession,
        position: LspPosition,
    ) -> Result<Vec<CompletionItem>, LspError> {
        let path = editor
            .active_path()
            .ok_or_else(|| LspError::new("No active file"))?;
        self.backend
            .completion(path, position, editor.active_revision())
    }

    pub fn request_completion(
        &self,
        editor: &EditorSession,
        position: LspPosition,
    ) -> Result<mpsc::Receiver<Result<Vec<CompletionItem>, LspError>>, LspError> {
        let path = editor
            .active_path()
            .ok_or_else(|| LspError::new("No active file"))?
            .to_owned();
        let revision = editor.active_revision();
        let backend = self.backend.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(backend.completion(&path, position, revision));
        });
        Ok(receiver)
    }

    pub fn definition(
        &self,
        editor: &EditorSession,
        position: LspPosition,
    ) -> Result<Option<DefinitionTarget>, LspError> {
        let path = editor
            .active_path()
            .ok_or_else(|| LspError::new("No active file"))?;
        self.backend
            .definition(path, position, editor.active_revision())
    }

    pub fn request_definition(
        &self,
        editor: &EditorSession,
        position: LspPosition,
    ) -> Result<mpsc::Receiver<Result<Option<DefinitionTarget>, LspError>>, LspError> {
        let path = editor
            .active_path()
            .ok_or_else(|| LspError::new("No active file"))?
            .to_owned();
        let revision = editor.active_revision();
        let backend = self.backend.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(backend.definition(&path, position, revision));
        });
        Ok(receiver)
    }

    pub fn shutdown(&mut self) -> Result<(), LspError> {
        self.backend.shutdown()?;
        self.started_workspace = None;
        self.attempted_workspace = None;
        self.synced_document = None;
        self.diagnostics.clear();
        self.diagnostics_receiver = None;
        self.last_error = None;
        Ok(())
    }

    pub fn backend(&self) -> Arc<dyn LanguageServerBackend> {
        self.backend.clone()
    }
}

pub struct WorkspaceSearchSession {
    backend: Arc<dyn WorkspaceSearchBackend>,
    cancelled: Option<Arc<AtomicBool>>,
    receiver: Option<mpsc::Receiver<Result<Vec<SearchHit>, SearchError>>>,
    query: String,
    hits: Vec<SearchHit>,
    running: bool,
    error: Option<String>,
}

impl WorkspaceSearchSession {
    pub fn new(backend: Arc<dyn WorkspaceSearchBackend>) -> Self {
        Self {
            backend,
            cancelled: None,
            receiver: None,
            query: String::new(),
            hits: Vec::new(),
            running: false,
            error: None,
        }
    }

    pub fn start(&mut self, root: PathBuf, query: String) {
        self.cancel();
        self.query = query.clone();
        self.hits.clear();
        self.error = None;
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        let backend = self.backend.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = backend.search(&root, &query, &worker_cancelled);
            let _ = sender.send(result);
        });
        self.cancelled = Some(cancelled);
        self.receiver = Some(receiver);
        self.running = true;
    }

    pub fn cancel(&mut self) {
        if let Some(cancelled) = self.cancelled.take() {
            cancelled.store(true, Ordering::Relaxed);
        }
        self.receiver = None;
        self.running = false;
    }

    pub fn refresh(&mut self) -> bool {
        let Some(receiver) = &self.receiver else {
            return false;
        };
        let Ok(result) = receiver.try_recv() else {
            return false;
        };
        match result {
            Ok(hits) => self.hits = hits,
            Err(error) => self.error = Some(error.to_string()),
        }
        self.receiver = None;
        self.cancelled = None;
        self.running = false;
        true
    }

    pub fn query(&self) -> &str {
        &self.query
    }
    pub fn hits(&self) -> &[SearchHit] {
        &self.hits
    }
    pub fn is_running(&self) -> bool {
        self.running
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn backend(&self) -> Arc<dyn WorkspaceSearchBackend> {
        self.backend.clone()
    }
}

/// UI と Task backend を分離する Command / Query 境界。
pub struct TaskSession {
    backend: Arc<dyn TaskBackend>,
    records: Vec<TaskRecord>,
    next_id: u64,
    restore_warning: Option<String>,
}

impl TaskSession {
    pub fn new(backend: Arc<dyn TaskBackend>) -> Self {
        let (records, restore_warning) = match backend.load() {
            Ok(records) => (records, None),
            Err(error) => (Vec::new(), Some(error.to_string())),
        };
        Self {
            backend,
            records,
            next_id: 0,
            restore_warning,
        }
    }

    pub fn records(&self) -> &[TaskRecord] {
        &self.records
    }

    pub fn restore_warning(&self) -> Option<&str> {
        self.restore_warning.as_deref()
    }

    pub fn refresh(&mut self) -> Result<bool, TaskError> {
        let records = self.backend.load()?;
        let changed = records != self.records;
        self.records = records;
        Ok(changed)
    }

    pub fn start_codex(
        &mut self,
        workspace_root: PathBuf,
        prompt: impl Into<String>,
    ) -> Result<ExecutionId, TaskError> {
        self.start_codex_with_mode(workspace_root, prompt, TaskMode::Default)
    }

    pub fn start_codex_with_mode(
        &mut self,
        workspace_root: PathBuf,
        prompt: impl Into<String>,
        mode: TaskMode,
    ) -> Result<ExecutionId, TaskError> {
        let prompt = prompt.into();
        if prompt.trim().is_empty() {
            return Err(TaskError::new("Task の指示が空です"));
        }
        if !workspace_root.is_dir() {
            return Err(TaskError::new(format!(
                "Task の Workspace が存在しません: {}",
                workspace_root.display()
            )));
        }
        self.next_id = self.next_id.saturating_add(1);
        let suffix = format!("{}-{}-{}", unix_time_ms(), std::process::id(), self.next_id);
        let task_id = TaskId::new(format!("task-{suffix}"));
        let execution_id = ExecutionId::new(format!("execution-{suffix}"));
        let title = prompt
            .lines()
            .next()
            .unwrap_or("Codex Task")
            .chars()
            .take(48)
            .collect::<String>();
        let record = TaskRecord::new(
            Task {
                id: task_id.clone(),
                conversation_id: ConversationId::new("conversation-local"),
                title,
                prompt,
                created_at_ms: unix_time_ms(),
            },
            Execution {
                id: execution_id.clone(),
                task_id,
                workspace_id: WorkspaceId::new("workspace-local"),
                workspace_root,
                runner: "codex-app-server".to_owned(),
                mode,
                status: ExecutionStatus::Queued,
                started_at_ms: None,
                completed_at_ms: None,
                external_thread_id: None,
                failure: None,
            },
        );
        self.backend.start(&record)?;
        self.records.insert(0, record);
        Ok(execution_id)
    }

    pub fn start_codex_in_worktree(
        &mut self,
        git: &mut GitSession,
        repository_root: PathBuf,
        prompt: impl Into<String>,
        mode: TaskMode,
    ) -> Result<ExecutionId, TaskError> {
        let prompt = prompt.into();
        if prompt.trim().is_empty() {
            return Err(TaskError::new("Task の指示が空です"));
        }
        self.next_id = self.next_id.saturating_add(1);
        let suffix = format!("{}-{}-{}", unix_time_ms(), std::process::id(), self.next_id);
        let task_id = TaskId::new(format!("task-{suffix}"));
        let execution_id = ExecutionId::new(format!("execution-{suffix}"));
        let worktree = git
            .create_task_worktree(&repository_root, &task_id)
            .map_err(|error| TaskError::new(error.to_string()))?;
        let title = prompt
            .lines()
            .next()
            .unwrap_or("Codex Task")
            .chars()
            .take(48)
            .collect();
        let record = TaskRecord::new(
            Task {
                id: task_id.clone(),
                conversation_id: ConversationId::new("conversation-local"),
                title,
                prompt,
                created_at_ms: unix_time_ms(),
            },
            Execution {
                id: execution_id.clone(),
                task_id,
                workspace_id: WorkspaceId::new(format!("worktree-{suffix}")),
                workspace_root: worktree.path,
                runner: "codex-app-server".to_owned(),
                mode,
                status: ExecutionStatus::Queued,
                started_at_ms: None,
                completed_at_ms: None,
                external_thread_id: None,
                failure: None,
            },
        );
        self.backend.start(&record)?;
        self.records.insert(0, record);
        Ok(execution_id)
    }

    pub fn control(
        &self,
        execution_id: &ExecutionId,
        control: TaskControl,
    ) -> Result<(), TaskError> {
        let record = self
            .records
            .iter()
            .find(|record| &record.execution.id == execution_id)
            .ok_or_else(|| TaskError::new("Task Execution が見つかりません"))?;
        if record.execution.status.is_terminal() {
            return Err(TaskError::new("終了済みの Task は操作できません"));
        }
        self.backend.control(execution_id, &control)
    }

    pub fn backend(&self) -> Arc<dyn TaskBackend> {
        self.backend.clone()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentTab {
    pub id: DocumentId,
    pub display_name: String,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    pub external_change: ExternalChange,
    pub encoding: Encoding,
    pub active: bool,
}

struct OpenDocument {
    id: DocumentId,
    document: Document,
    external_change: ExternalChange,
    view: DocumentViewState,
}

/// UIからDocumentと外部I/Oを分離する、複数文書Workspace編集セッション。
pub struct EditorSession {
    documents: Vec<OpenDocument>,
    active: Option<usize>,
    workspace_root: Option<PathBuf>,
    workspace_name: String,
    file_tree: Vec<FileEntry>,
    next_document_id: u64,
    repository: Arc<dyn WorkspaceRepository>,
    file_dialog: Arc<dyn WorkspaceDialog>,
    state_repository: Arc<dyn WorkspaceStateRepository>,
    restore_warning: Option<String>,
}

impl EditorSession {
    pub fn new(
        repository: Arc<dyn WorkspaceRepository>,
        file_dialog: Arc<dyn WorkspaceDialog>,
        state_repository: Arc<dyn WorkspaceStateRepository>,
    ) -> Self {
        let mut session = Self::new_empty(repository, file_dialog, state_repository);
        if let Err(error) = session.restore() {
            session.restore_warning = Some(error.to_string());
        }
        if session.documents.is_empty() {
            session.new_document();
        }
        session
    }

    /// Creates a session for a new window without restoring a workspace or document.
    pub fn new_empty(
        repository: Arc<dyn WorkspaceRepository>,
        file_dialog: Arc<dyn WorkspaceDialog>,
        state_repository: Arc<dyn WorkspaceStateRepository>,
    ) -> Self {
        Self {
            documents: Vec::new(),
            active: None,
            workspace_root: None,
            workspace_name: "No Workspace".to_owned(),
            file_tree: Vec::new(),
            next_document_id: 1,
            repository,
            file_dialog,
            state_repository,
            restore_warning: None,
        }
    }

    pub fn restore_warning(&self) -> Option<&str> {
        self.restore_warning.as_deref()
    }

    pub fn workspace_root(&self) -> Option<&Path> {
        self.workspace_root.as_deref()
    }

    pub fn workspace_name(&self) -> &str {
        &self.workspace_name
    }

    pub fn snapshot(&self) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            root: self.workspace_root.clone(),
            open_documents: self
                .documents
                .iter()
                .map(|open| DocumentViewState {
                    document_id: Some(open.id.clone()),
                    path: open.document.path().map(Path::to_owned).unwrap_or_default(),
                    cursor_char: open.view.cursor_char,
                    selection_start: open.view.selection_start,
                    selection_end: open.view.selection_end,
                    scroll_x: open.view.scroll_x,
                    scroll_y: open.view.scroll_y,
                    draft_content: open
                        .document
                        .is_dirty()
                        .then(|| open.document.content_for_save()),
                })
                .collect(),
            active_path: self
                .active_document()
                .and_then(|open| open.document.path().map(Path::to_owned)),
        }
    }

    pub fn file_tree(&self) -> &[FileEntry] {
        &self.file_tree
    }

    pub fn tabs(&self) -> Vec<DocumentTab> {
        self.documents
            .iter()
            .enumerate()
            .map(|(index, open)| DocumentTab {
                id: open.id.clone(),
                display_name: if open.document.path().is_none() {
                    format!("Untitled {}", open.id.as_str())
                } else {
                    open.document.display_name()
                },
                path: open.document.path().map(Path::to_owned),
                dirty: open.document.is_dirty(),
                external_change: open.external_change,
                encoding: open.document.encoding(),
                active: self.active == Some(index),
            })
            .collect()
    }

    pub fn active_document_id(&self) -> Option<&DocumentId> {
        self.active_document().map(|open| &open.id)
    }

    pub fn active_path(&self) -> Option<&Path> {
        self.active_document()?.document.path()
    }

    pub fn active_revision(&self) -> Revision {
        self.active_document()
            .map(|open| open.document.revision())
            .unwrap_or_default()
    }

    pub fn active_text(&self) -> Result<String, DocumentError> {
        let document = &self.required_document()?.document;
        document.slice_chars(0..document.len_chars())
    }

    pub fn display_name(&self) -> String {
        self.active_document()
            .map(|open| open.document.display_name())
            .unwrap_or_else(|| "Untitled".to_owned())
    }

    pub fn revision(&self) -> u64 {
        self.active_document()
            .map(|open| open.document.revision().number())
            .unwrap_or_default()
    }

    pub fn encoding(&self) -> Encoding {
        self.active_document()
            .map(|open| open.document.encoding())
            .unwrap_or_default()
    }

    pub fn is_dirty(&self) -> bool {
        self.active_document()
            .is_some_and(|open| open.document.is_dirty())
    }

    pub fn external_change(&self) -> ExternalChange {
        self.active_document()
            .map(|open| open.external_change)
            .unwrap_or(ExternalChange::Unchanged)
    }

    pub fn has_external_change(&self) -> bool {
        self.external_change() != ExternalChange::Unchanged
    }

    pub fn len_chars(&self) -> usize {
        self.active_document()
            .map(|open| open.document.len_chars())
            .unwrap_or_default()
    }

    pub fn len_bytes(&self) -> usize {
        self.active_document()
            .map(|open| open.document.len_bytes())
            .unwrap_or_default()
    }

    pub fn len_lines(&self) -> usize {
        self.active_document()
            .map(|open| open.document.len_lines())
            .unwrap_or(1)
    }

    pub fn is_empty(&self) -> bool {
        self.active_document()
            .is_none_or(|open| open.document.is_empty())
    }

    pub fn line(&self, line: usize) -> Option<String> {
        self.active_document()?.document.line(line)
    }

    pub fn line_start_char(&self, line: usize) -> Result<usize, DocumentError> {
        self.required_document()?
            .document
            .position_to_char(Position::new(
                u32::try_from(line).map_err(|_| {
                    DocumentError::new(
                        lapis_document::DocumentErrorKind::InvalidRange,
                        "line index is too large",
                    )
                })?,
                0,
            ))
    }

    pub fn slice_chars(&self, range: Range<usize>) -> Result<String, DocumentError> {
        self.required_document()?.document.slice_chars(range)
    }

    pub fn char_to_byte(&self, char_index: usize) -> Result<usize, DocumentError> {
        self.required_document()?.document.char_to_byte(char_index)
    }

    pub fn byte_to_char(&self, byte_index: usize) -> Result<usize, DocumentError> {
        self.required_document()?.document.byte_to_char(byte_index)
    }

    pub fn char_to_position(&self, char_index: usize) -> Result<Position, DocumentError> {
        self.required_document()?
            .document
            .char_to_position(char_index)
    }

    pub fn position_to_char(&self, position: Position) -> Result<usize, DocumentError> {
        self.required_document()?
            .document
            .position_to_char(position)
    }

    pub fn char_to_utf16_offset(&self, char_index: usize) -> Result<usize, DocumentError> {
        self.required_document()?
            .document
            .char_to_utf16_offset(char_index)
    }

    pub fn utf16_offset_to_char(&self, offset: usize) -> Result<usize, DocumentError> {
        self.required_document()?
            .document
            .utf16_offset_to_char(offset)
    }

    pub fn replace_range(
        &mut self,
        range: Range<usize>,
        replacement: &str,
    ) -> Result<bool, DocumentError> {
        let changed = self
            .required_document_mut()?
            .document
            .replace_char_range(range, replacement)?;
        if changed {
            self.persist_best_effort();
        }
        Ok(changed)
    }

    pub fn undo(&mut self) -> bool {
        let changed = self
            .active_document_mut()
            .is_some_and(|open| open.document.undo());
        if changed {
            self.persist_best_effort();
        }
        changed
    }

    pub fn redo(&mut self) -> bool {
        let changed = self
            .active_document_mut()
            .is_some_and(|open| open.document.redo());
        if changed {
            self.persist_best_effort();
        }
        changed
    }

    pub fn can_undo(&self) -> bool {
        self.active_document()
            .is_some_and(|open| open.document.can_undo())
    }

    pub fn can_redo(&self) -> bool {
        self.active_document()
            .is_some_and(|open| open.document.can_redo())
    }

    pub fn find(&self, query: &str, case_sensitive: bool) -> Vec<Range<usize>> {
        self.active_document()
            .map(|open| open.document.find(query, case_sensitive))
            .unwrap_or_default()
    }

    pub fn active_view(&self) -> Option<&DocumentViewState> {
        self.active_document().map(|open| &open.view)
    }

    pub fn update_active_view(
        &mut self,
        selection: Range<usize>,
        cursor: usize,
        scroll_x: f32,
        scroll_y: f32,
    ) {
        if let Some(open) = self.active_document_mut() {
            open.view.selection_start = selection.start;
            open.view.selection_end = selection.end;
            open.view.cursor_char = cursor;
            open.view.scroll_x = scroll_x;
            open.view.scroll_y = scroll_y;
        }
        self.persist_best_effort();
    }

    pub fn new_document(&mut self) {
        let id = self.allocate_document_id();
        let view = DocumentViewState {
            document_id: Some(id.clone()),
            ..DocumentViewState::default()
        };
        self.documents.push(OpenDocument {
            id,
            document: Document::new(),
            external_change: ExternalChange::Unchanged,
            view,
        });
        self.active = Some(self.documents.len() - 1);
        self.persist_best_effort();
    }

    pub fn choose_workspace(&mut self) -> Result<DocumentAction, WorkspaceError> {
        let Some(root) = self.file_dialog.choose_workspace_path() else {
            return Ok(DocumentAction::Cancelled);
        };
        self.open_workspace(root)?;
        Ok(DocumentAction::Completed)
    }

    pub fn choose_file(&mut self) -> Result<DocumentAction, DocumentError> {
        let Some(path) = self.file_dialog.choose_file_path() else {
            return Ok(DocumentAction::Cancelled);
        };
        self.open_path(path)?;
        Ok(DocumentAction::Completed)
    }

    pub fn open_workspace(&mut self, root: PathBuf) -> Result<(), WorkspaceError> {
        let tree = self.repository.list_tree(&root)?;
        self.workspace_name = root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Workspace")
            .to_owned();
        self.workspace_root = Some(root);
        self.file_tree = tree;
        self.documents.clear();
        self.active = None;
        self.persist()?;
        Ok(())
    }

    pub fn close_workspace(&mut self) -> Result<(), WorkspaceError> {
        self.workspace_root = None;
        self.workspace_name = "No Workspace".to_owned();
        self.file_tree.clear();
        self.documents.clear();
        self.active = None;
        self.persist()?;
        Ok(())
    }

    pub fn repository(&self) -> Arc<dyn WorkspaceRepository> {
        self.repository.clone()
    }

    pub fn file_dialog(&self) -> Arc<dyn WorkspaceDialog> {
        self.file_dialog.clone()
    }

    pub fn state_repository(&self) -> Arc<dyn WorkspaceStateRepository> {
        self.state_repository.clone()
    }

    pub fn refresh_file_tree(&mut self) -> Result<(), WorkspaceError> {
        let Some(root) = self.workspace_root.as_deref() else {
            return Ok(());
        };
        self.file_tree = self.repository.list_tree(root)?;
        Ok(())
    }

    pub fn open_path(&mut self, path: PathBuf) -> Result<(), DocumentError> {
        if let Some(index) = self
            .documents
            .iter()
            .position(|open| open.document.path() == Some(path.as_path()))
        {
            self.active = Some(index);
            self.persist_best_effort();
            return Ok(());
        }
        let data = self.repository.read_file(&path)?;
        let document = Document::from_file(path.clone(), data)?;
        let id = self.allocate_document_id();
        self.documents.push(OpenDocument {
            id: id.clone(),
            document,
            external_change: ExternalChange::Unchanged,
            view: DocumentViewState {
                document_id: Some(id),
                path,
                ..DocumentViewState::default()
            },
        });
        self.active = Some(self.documents.len() - 1);
        self.persist_best_effort();
        Ok(())
    }

    pub fn open_definition(&mut self, target: &DefinitionTarget) -> Result<usize, DocumentError> {
        self.open_path(target.path.clone())?;
        self.position_to_char(Position {
            line: target.range.start.line,
            utf16_column: target.range.start.utf16_column,
        })
    }

    pub fn activate_document(&mut self, id: &DocumentId) -> bool {
        let Some(index) = self.documents.iter().position(|open| &open.id == id) else {
            return false;
        };
        self.active = Some(index);
        self.persist_best_effort();
        true
    }

    pub fn close_document(
        &mut self,
        id: &DocumentId,
        disposition: DocumentCloseDisposition,
    ) -> Result<bool, DocumentError> {
        let Some(index) = self.documents.iter().position(|open| &open.id == id) else {
            return Ok(false);
        };
        if self.documents[index].document.is_dirty()
            && disposition == DocumentCloseDisposition::PreserveChanges
        {
            return Err(DocumentError::conflict(
                "未保存の文書は確認なしに閉じられません",
            ));
        }
        let was_active = self.active == Some(index);
        self.documents.remove(index);
        self.active = if self.documents.is_empty() {
            None
        } else if was_active {
            Some(index.min(self.documents.len() - 1))
        } else {
            self.active
                .map(|active| active.saturating_sub(usize::from(active > index)))
        };
        self.persist_best_effort();
        Ok(true)
    }

    pub fn save_document(&mut self) -> Result<DocumentAction, DocumentError> {
        let path = if let Some(path) = self.required_document()?.document.path() {
            path.to_owned()
        } else {
            let Some(path) = self.file_dialog.choose_save_path(&self.display_name()) else {
                return Ok(DocumentAction::Cancelled);
            };
            path
        };

        let (bytes, expected) = {
            let open = self.required_document()?;
            (
                open.document.encoded_bytes(),
                open.document.saved_fingerprint().cloned(),
            )
        };
        let fingerprint = self
            .repository
            .write_file(&path, &bytes, expected.as_ref())?;
        let open = self.required_document_mut()?;
        open.document.mark_saved(path, fingerprint);
        open.external_change = ExternalChange::Unchanged;
        self.persist_best_effort();
        Ok(DocumentAction::Completed)
    }

    pub fn reload_active_from_disk(&mut self) -> Result<(), DocumentError> {
        let path = self
            .required_document()?
            .document
            .path()
            .ok_or_else(|| DocumentError::io("未保存文書は再読み込みできません"))?
            .to_owned();
        if self.required_document()?.document.is_dirty() {
            return Err(DocumentError::conflict(
                "未保存変更がある文書は確認なしに再読み込みできません",
            ));
        }
        let data = self.repository.read_file(&path)?;
        let document = Document::from_file(path, data)?;
        let open = self.required_document_mut()?;
        open.document = document;
        open.external_change = ExternalChange::Unchanged;
        self.persist_best_effort();
        Ok(())
    }

    pub fn poll_external_changes(&mut self) -> Vec<(DocumentId, ExternalChange)> {
        let repository = self.repository.clone();
        let mut changes = Vec::new();
        for open in &mut self.documents {
            let Some(path) = open.document.path() else {
                continue;
            };
            let actual = match repository.fingerprint(path) {
                Ok(value) => value,
                Err(_) => continue,
            };
            let state = open.document.external_change(actual.as_ref());
            if state != open.external_change {
                open.external_change = state;
                changes.push((open.id.clone(), state));
            }
        }
        changes
    }

    fn required_document(&self) -> Result<&OpenDocument, DocumentError> {
        self.active_document()
            .ok_or_else(|| DocumentError::io("開いている文書がありません"))
    }

    fn required_document_mut(&mut self) -> Result<&mut OpenDocument, DocumentError> {
        self.active_document_mut()
            .ok_or_else(|| DocumentError::io("開いている文書がありません"))
    }

    fn active_document(&self) -> Option<&OpenDocument> {
        self.active.and_then(|index| self.documents.get(index))
    }

    fn active_document_mut(&mut self) -> Option<&mut OpenDocument> {
        self.active.and_then(|index| self.documents.get_mut(index))
    }

    fn allocate_document_id(&mut self) -> DocumentId {
        let id = DocumentId::new(format!("document-{}", self.next_document_id));
        self.next_document_id = self.next_document_id.saturating_add(1);
        id
    }

    fn restore(&mut self) -> Result<(), WorkspaceError> {
        let Some(snapshot) = self.state_repository.load()? else {
            return Ok(());
        };
        self.restore_snapshot(snapshot)
    }

    pub fn restore_snapshot(&mut self, snapshot: WorkspaceSnapshot) -> Result<(), WorkspaceError> {
        self.documents.clear();
        self.active = None;
        self.workspace_root = None;
        self.file_tree.clear();
        self.workspace_name = "No Workspace".to_owned();
        if let Some(root) = snapshot.root {
            self.workspace_name = root
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("Workspace")
                .to_owned();
            self.file_tree = self.repository.list_tree(&root)?;
            self.workspace_root = Some(root);
        }
        for view in snapshot.open_documents {
            let mut document = if view.path.as_os_str().is_empty() {
                Document::new()
            } else {
                let Ok(data) = self.repository.read_file(&view.path) else {
                    continue;
                };
                let Ok(document) = Document::from_file(view.path.clone(), data) else {
                    continue;
                };
                document
            };
            if let Some(draft) = &view.draft_content {
                let len = document.len_chars();
                if document.replace_char_range(0..len, draft).is_err() {
                    continue;
                }
            }
            let id = self.allocate_document_id();
            self.documents.push(OpenDocument {
                id: id.clone(),
                document,
                external_change: ExternalChange::Unchanged,
                view: DocumentViewState {
                    document_id: Some(id),
                    ..view
                },
            });
        }
        self.active = snapshot.active_path.and_then(|path| {
            self.documents
                .iter()
                .position(|open| open.document.path() == Some(path.as_path()))
        });
        if self.active.is_none() && !self.documents.is_empty() {
            self.active = Some(0);
        }
        Ok(())
    }

    fn persist_best_effort(&mut self) {
        if let Err(error) = self.persist() {
            self.restore_warning = Some(error.to_string());
        }
    }

    fn persist(&self) -> Result<(), WorkspaceError> {
        self.state_repository.save(&self.snapshot())
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Mutex};

    use lapis_document::{DocumentErrorKind, FileData, FileFingerprint};
    use lapis_workspace::{FileEntryKind, WorkspaceStateRepository};

    use super::*;

    #[derive(Default)]
    struct MemoryRepository(Mutex<HashMap<PathBuf, FileData>>);

    struct FailingSettingsRepository(Mutex<GlobalSettings>);

    impl GlobalSettingsRepository for FailingSettingsRepository {
        fn load(&self) -> Result<GlobalSettings, SettingsError> {
            Ok(self.0.lock().unwrap().clone())
        }

        fn save(&self, _: &GlobalSettings) -> Result<(), SettingsError> {
            Err(SettingsError::new("settings storage unavailable"))
        }
    }

    impl DocumentRepository for MemoryRepository {
        fn read_file(&self, path: &Path) -> Result<FileData, DocumentError> {
            self.0
                .lock()
                .unwrap()
                .get(path)
                .cloned()
                .ok_or_else(|| DocumentError::io("not found"))
        }

        fn write_file(
            &self,
            path: &Path,
            content: &[u8],
            expected: Option<&FileFingerprint>,
        ) -> Result<FileFingerprint, DocumentError> {
            let mut files = self.0.lock().unwrap();
            if let Some(current) = files.get(path)
                && Some(&current.fingerprint) != expected
            {
                return Err(DocumentError::conflict("changed"));
            }
            let fingerprint = test_fingerprint(content);
            files.insert(
                path.to_owned(),
                FileData {
                    bytes: content.to_vec(),
                    fingerprint: fingerprint.clone(),
                },
            );
            Ok(fingerprint)
        }

        fn fingerprint(&self, path: &Path) -> Result<Option<FileFingerprint>, DocumentError> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .get(path)
                .map(|data| data.fingerprint.clone()))
        }
    }

    impl WorkspaceRepository for MemoryRepository {
        fn list_tree(&self, root: &Path) -> Result<Vec<FileEntry>, WorkspaceError> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .keys()
                .map(|path| FileEntry {
                    path: path.clone(),
                    relative_path: path.strip_prefix(root).unwrap_or(path).to_owned(),
                    kind: FileEntryKind::File,
                    depth: 0,
                })
                .collect())
        }
    }

    #[derive(Default)]
    struct MemoryState(Mutex<Option<WorkspaceSnapshot>>);

    impl WorkspaceStateRepository for MemoryState {
        fn load(&self) -> Result<Option<WorkspaceSnapshot>, WorkspaceError> {
            Ok(self.0.lock().unwrap().clone())
        }

        fn save(&self, snapshot: &WorkspaceSnapshot) -> Result<(), WorkspaceError> {
            *self.0.lock().unwrap() = Some(snapshot.clone());
            Ok(())
        }
    }

    #[derive(Default)]
    struct MemoryConversationState(Mutex<(Vec<ConversationRecord>, Option<ConversationId>)>);

    impl ConversationRepository for MemoryConversationState {
        fn load(
            &self,
        ) -> Result<(Vec<ConversationRecord>, Option<ConversationId>), WorkspaceError> {
            Ok(self.0.lock().unwrap().clone())
        }

        fn save(
            &self,
            records: &[ConversationRecord],
            active: &ConversationId,
        ) -> Result<(), WorkspaceError> {
            *self.0.lock().unwrap() = (records.to_vec(), Some(active.clone()));
            Ok(())
        }
    }

    #[derive(Default)]
    struct MemoryTaskBackend {
        records: Mutex<Vec<TaskRecord>>,
        controls: Mutex<Vec<(ExecutionId, TaskControl)>>,
    }

    struct UnavailableTerminalBackend;

    impl TerminalBackend for UnavailableTerminalBackend {
        fn start(
            &self,
            _cwd: &Path,
            _columns: u16,
            _rows: u16,
        ) -> Result<TerminalId, TerminalError> {
            Err(TerminalError::new("unavailable"))
        }

        fn input(&self, _id: &TerminalId, _bytes: &[u8]) -> Result<(), TerminalError> {
            Err(TerminalError::new("unavailable"))
        }

        fn resize(&self, _id: &TerminalId, _columns: u16, _rows: u16) -> Result<(), TerminalError> {
            Err(TerminalError::new("unavailable"))
        }

        fn poll(&self, _id: &TerminalId) -> Result<Vec<TerminalEvent>, TerminalError> {
            Err(TerminalError::new("unavailable"))
        }

        fn terminate(&self, _id: &TerminalId) -> Result<(), TerminalError> {
            Err(TerminalError::new("unavailable"))
        }
    }

    struct ScriptedTerminalBackend {
        events: Mutex<Vec<TerminalEvent>>,
    }

    impl TerminalBackend for ScriptedTerminalBackend {
        fn start(
            &self,
            _cwd: &Path,
            _columns: u16,
            _rows: u16,
        ) -> Result<TerminalId, TerminalError> {
            Ok(TerminalId::new("scripted-terminal"))
        }

        fn input(&self, _id: &TerminalId, _bytes: &[u8]) -> Result<(), TerminalError> {
            Ok(())
        }

        fn resize(&self, _id: &TerminalId, _columns: u16, _rows: u16) -> Result<(), TerminalError> {
            Ok(())
        }

        fn poll(&self, _id: &TerminalId) -> Result<Vec<TerminalEvent>, TerminalError> {
            Ok(self.events.lock().unwrap().drain(..).collect())
        }

        fn terminate(&self, _id: &TerminalId) -> Result<(), TerminalError> {
            Ok(())
        }
    }

    impl TaskBackend for MemoryTaskBackend {
        fn load(&self) -> Result<Vec<TaskRecord>, TaskError> {
            Ok(self.records.lock().unwrap().clone())
        }

        fn start(&self, record: &TaskRecord) -> Result<(), TaskError> {
            self.records.lock().unwrap().insert(0, record.clone());
            Ok(())
        }

        fn control(
            &self,
            execution_id: &ExecutionId,
            control: &TaskControl,
        ) -> Result<(), TaskError> {
            self.controls
                .lock()
                .unwrap()
                .push((execution_id.clone(), control.clone()));
            Ok(())
        }
    }

    struct FixedDialog {
        workspace: Option<PathBuf>,
        save: Option<PathBuf>,
    }

    impl WorkspaceDialog for FixedDialog {
        fn choose_workspace_path(&self) -> Option<PathBuf> {
            self.workspace.clone()
        }

        fn choose_save_path(&self, _suggested_name: &str) -> Option<PathBuf> {
            self.save.clone()
        }
    }

    fn test_fingerprint(bytes: &[u8]) -> FileFingerprint {
        FileFingerprint::new(
            bytes.len() as u64,
            Some(bytes.len() as u128),
            bytes.len() as u64,
        )
    }

    #[test]
    fn setting_theme_rolls_back_when_persistence_fails() {
        let repository = Arc::new(FailingSettingsRepository(Mutex::new(
            GlobalSettings::default(),
        )));
        let settings = SettingsSession::load(repository).unwrap();

        assert!(settings.set_theme("lapis.white".to_owned()).is_err());
        assert_eq!(settings.settings().theme, "lapis.dark");
    }

    fn insert_file(repository: &MemoryRepository, path: PathBuf, content: &str) {
        repository.0.lock().unwrap().insert(
            path,
            FileData {
                bytes: content.as_bytes().to_vec(),
                fingerprint: test_fingerprint(content.as_bytes()),
            },
        );
    }

    fn session(repository: Arc<MemoryRepository>, state: Arc<MemoryState>) -> EditorSession {
        EditorSession::new(
            repository,
            Arc::new(FixedDialog {
                workspace: None,
                save: None,
            }),
            state,
        )
    }
    #[test]
    fn new_empty_starts_without_workspace_or_document() {
        let editor = EditorSession::new_empty(
            Arc::new(MemoryRepository::default()),
            Arc::new(FixedDialog {
                workspace: None,
                save: None,
            }),
            Arc::new(MemoryState::default()),
        );

        assert!(editor.workspace_root().is_none());
        assert!(editor.tabs().is_empty());
    }

    #[test]
    fn opens_and_switches_three_documents() {
        let repository = Arc::new(MemoryRepository::default());
        let state = Arc::new(MemoryState::default());
        for name in ["one.rs", "two.md", "three.txt"] {
            insert_file(&repository, PathBuf::from(name), name);
        }
        let mut editor = session(repository, state);
        for name in ["one.rs", "two.md", "three.txt"] {
            editor.open_path(PathBuf::from(name)).unwrap();
        }
        assert!(editor.tabs().len() >= 3);
        let first = editor.tabs()[1].id.clone();
        assert!(editor.activate_document(&first));
        assert_eq!(editor.active_document_id(), Some(&first));
    }

    #[test]
    fn closing_active_document_selects_the_previous_document() {
        let repository = Arc::new(MemoryRepository::default());
        let state = Arc::new(MemoryState::default());
        for name in ["one.rs", "two.md", "three.txt"] {
            insert_file(&repository, PathBuf::from(name), name);
        }
        let mut editor = session(repository, state);
        for name in ["one.rs", "two.md", "three.txt"] {
            editor.open_path(PathBuf::from(name)).unwrap();
        }
        let tabs = editor.tabs();
        let active = tabs.iter().find(|tab| tab.active).unwrap().id.clone();
        let previous = tabs[tabs.len() - 2].id.clone();

        assert!(
            editor
                .close_document(&active, DocumentCloseDisposition::PreserveChanges)
                .unwrap()
        );
        assert_eq!(editor.active_document_id(), Some(&previous));
    }

    #[test]
    fn closing_inactive_document_keeps_the_active_document() {
        let repository = Arc::new(MemoryRepository::default());
        let state = Arc::new(MemoryState::default());
        for name in ["one.rs", "two.md", "three.txt"] {
            insert_file(&repository, PathBuf::from(name), name);
        }
        let mut editor = session(repository, state);
        for name in ["one.rs", "two.md", "three.txt"] {
            editor.open_path(PathBuf::from(name)).unwrap();
        }
        let tabs = editor.tabs();
        let inactive = tabs.iter().find(|tab| !tab.active).unwrap().id.clone();
        let active = tabs.iter().find(|tab| tab.active).unwrap().id.clone();
        assert_eq!(editor.active_document_id(), Some(&active));

        assert!(
            editor
                .close_document(&inactive, DocumentCloseDisposition::PreserveChanges)
                .unwrap()
        );
        assert_eq!(editor.active_document_id(), Some(&active));
    }

    #[test]
    fn dirty_document_requires_disposition_before_close() {
        let repository = Arc::new(MemoryRepository::default());
        let path = PathBuf::from("note.md");
        insert_file(&repository, path.clone(), "base");
        let mut editor = session(repository, Arc::new(MemoryState::default()));
        editor.open_path(path).unwrap();
        let id = editor.active_document_id().unwrap().clone();
        editor.replace_range(0..4, "draft").unwrap();

        assert_eq!(
            editor
                .close_document(&id, DocumentCloseDisposition::PreserveChanges)
                .unwrap_err()
                .kind(),
            DocumentErrorKind::Conflict
        );
        assert!(
            editor
                .close_document(&id, DocumentCloseDisposition::DiscardChanges)
                .unwrap()
        );
        assert!(editor.tabs().iter().all(|tab| tab.id != id));
    }

    #[test]
    fn save_detects_external_conflict() {
        let repository = Arc::new(MemoryRepository::default());
        let path = PathBuf::from("note.md");
        insert_file(&repository, path.clone(), "old");
        let mut editor = session(repository.clone(), Arc::new(MemoryState::default()));
        editor.open_path(path.clone()).unwrap();
        editor.replace_range(0..3, "local").unwrap();
        insert_file(&repository, path, "external");

        let error = editor.save_document().unwrap_err();
        assert_eq!(error.kind(), DocumentErrorKind::Conflict);
    }

    #[test]
    fn workspace_and_dirty_documents_restore() {
        let repository = Arc::new(MemoryRepository::default());
        let state = Arc::new(MemoryState::default());
        let root = PathBuf::from("repo");
        let path = root.join("note.md");
        insert_file(&repository, path.clone(), "base");
        {
            let mut editor = session(repository.clone(), state.clone());
            editor.open_workspace(root.clone()).unwrap();
            editor.open_path(path.clone()).unwrap();
            editor.replace_range(0..4, "draft😀").unwrap();
        }

        let restored = session(repository, state);
        assert_eq!(restored.workspace_root(), Some(root.as_path()));
        assert!(restored.is_dirty());
        assert_eq!(restored.slice_chars(0..6).unwrap(), "draft😀");
    }

    #[test]
    fn conversations_switch_independent_workspace_snapshots() {
        let repository = Arc::new(MemoryRepository::default());
        let root = PathBuf::from("repo");
        let first_path = root.join("first.md");
        let second_path = root.join("second.md");
        insert_file(&repository, first_path.clone(), "first");
        insert_file(&repository, second_path.clone(), "second");
        let mut editor = session(repository, Arc::new(MemoryState::default()));
        editor.open_workspace(root).unwrap();
        editor.open_path(first_path).unwrap();

        let store = Arc::new(MemoryConversationState::default());
        let mut conversations = ConversationSession::new(store.clone(), editor.snapshot());
        let first = conversations.active_id().clone();
        conversations
            .capture(&editor, ConversationViewState::default(), None, &[])
            .unwrap();
        let second = conversations
            .create(
                &editor,
                ConversationViewState {
                    active_tool: "search".to_owned(),
                    ..ConversationViewState::default()
                },
            )
            .unwrap();
        editor.open_path(second_path).unwrap();
        conversations
            .capture(&editor, conversations.active_view(), None, &[])
            .unwrap();

        let first_view = conversations.switch(&first, &mut editor).unwrap();
        assert_eq!(first_view.active_tool, "files");
        assert_eq!(editor.tabs().len(), 1);
        assert_eq!(editor.display_name(), "first.md");

        let second_view = conversations.switch(&second, &mut editor).unwrap();
        assert_eq!(second_view.active_tool, "search");
        assert_eq!(editor.tabs().len(), 2);
        assert_eq!(editor.display_name(), "second.md");

        let persisted = store.load().unwrap();
        assert_eq!(persisted.0.len(), 2);
        assert_eq!(persisted.1, Some(second));
    }

    #[test]
    fn restored_terminal_summaries_never_resume_processes_or_output() {
        let backend = Arc::new(UnavailableTerminalBackend);
        let mut terminals = TerminalSession::new(backend);
        terminals.restore_summaries(&[RestoredTerminal {
            cwd: PathBuf::from("repo"),
            status: TerminalStatus::Running,
            columns: 120,
            rows: 30,
        }]);

        assert_eq!(terminals.terminals().len(), 1);
        assert_eq!(terminals.terminals()[0].status, TerminalStatus::Exited);
        assert!(terminals.terminals()[0].output.is_empty());
        assert!(!terminals.refresh().unwrap());
    }

    #[test]
    fn terminal_session_keeps_ordered_raw_output_and_watermark() {
        let backend = Arc::new(ScriptedTerminalBackend {
            events: Mutex::new(vec![
                TerminalEvent::Output(lapis_terminal::TerminalOutput {
                    sequence: 1,
                    data: b"\x1b[31mred\x1b[0m".to_vec(),
                }),
                TerminalEvent::Output(lapis_terminal::TerminalOutput {
                    sequence: 2,
                    data: b"\x1b[2;4H".to_vec(),
                }),
                TerminalEvent::Output(lapis_terminal::TerminalOutput {
                    sequence: 3,
                    data: b"\x1b[2J".to_vec(),
                }),
                TerminalEvent::Output(lapis_terminal::TerminalOutput {
                    sequence: 4,
                    data: vec![0xe3],
                }),
                TerminalEvent::Output(lapis_terminal::TerminalOutput {
                    sequence: 5,
                    data: vec![0x81, 0x82],
                }),
                TerminalEvent::Output(lapis_terminal::TerminalOutput {
                    sequence: 6,
                    data: vec![0xff, 0x00],
                }),
            ]),
        });
        let mut terminals = TerminalSession::new(backend);
        terminals.start(Path::new("repo"), 120, 30).unwrap();

        assert!(terminals.refresh().unwrap());
        let terminal = &terminals.terminals()[0];
        assert_eq!(
            terminal.output,
            b"\x1b[31mred\x1b[0m\x1b[2;4H\x1b[2J\xe3\x81\x82\xff\x00"
        );
        assert_eq!(terminal.output_sequence, 6);
        assert!(!terminal.output_truncated);
    }

    #[test]
    fn task_session_starts_plan_refreshes_and_routes_control() {
        let workspace = tempfile::tempdir().unwrap();
        let backend = Arc::new(MemoryTaskBackend::default());
        let mut tasks = TaskSession::new(backend.clone());
        let execution_id = tasks
            .start_codex_with_mode(workspace.path().to_owned(), "質問する", TaskMode::Plan)
            .unwrap();

        assert_eq!(tasks.records().len(), 1);
        assert_eq!(tasks.records()[0].execution.mode, TaskMode::Plan);
        assert_eq!(tasks.records()[0].execution.status, ExecutionStatus::Queued);

        backend.records.lock().unwrap()[0].set_status(ExecutionStatus::WaitingForInput);
        assert!(tasks.refresh().unwrap());
        tasks
            .control(
                &execution_id,
                TaskControl::Reply {
                    text: "青".to_owned(),
                },
            )
            .unwrap();
        assert_eq!(backend.controls.lock().unwrap().len(), 1);
    }
}
