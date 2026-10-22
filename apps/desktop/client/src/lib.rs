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
    Document, DocumentCache, DocumentConnection, DocumentError, ExternalChange, Position, Revision,
};
use lapis_editor_core::{ConversationId, DocumentId, ExecutionId, TaskId, WorkspaceId};
use lapis_git::GitError;
use lapis_localization::LocaleId;
use lapis_lsp::{
    CompletionItem, DefinitionTarget, Diagnostic, LanguageServerBackend, LspError, LspPosition,
};
use lapis_settings::{GlobalSettings, GlobalSettingsRepository, SettingsError};
use lapis_task::{TaskControl, TaskError, TaskMode};
use lapis_terminal::{
    TerminalBackend, TerminalError, TerminalEvent, TerminalId, TerminalSnapshot, TerminalStatus,
};
use lapis_workspace::{FileEntry, SearchError, SearchHit, WorkspaceError, WorkspaceSearchBackend};
use serde::{Deserialize, Serialize};

mod view_state;
pub use view_state::{DocumentViewState, WorkspaceSnapshot, WorkspaceStateRepository};

/// グローバル設定を UI に公開し、変更を永続化するアプリケーション境界。
#[derive(Clone)]
pub struct SettingsSession {
    repository: Arc<dyn GlobalSettingsRepository>,
    settings: Arc<std::sync::Mutex<GlobalSettings>>,
    writer: Arc<std::sync::Mutex<()>>,
}

impl SettingsSession {
    pub fn load(repository: Arc<dyn GlobalSettingsRepository>) -> Result<Self, SettingsError> {
        let settings = repository.load()?;
        Ok(Self {
            repository,
            settings: Arc::new(std::sync::Mutex::new(settings)),
            writer: Arc::new(std::sync::Mutex::new(())),
        })
    }

    pub fn settings(&self) -> GlobalSettings {
        self.settings
            .lock()
            .expect("settings state lock failed")
            .clone()
    }

    pub fn set_locale(&self, locale: LocaleId) -> Result<(), SettingsError> {
        let _writer = self.writer.lock().expect("settings writer lock failed");
        let mut settings = self.settings();
        if settings.locale == locale {
            return Ok(());
        }
        settings.locale = locale;
        self.repository.save(&settings)?;
        *self.settings.lock().expect("settings state lock failed") = settings;
        Ok(())
    }

    pub fn set_theme(&self, theme: String) -> Result<(), SettingsError> {
        let _writer = self.writer.lock().expect("settings writer lock failed");
        let mut settings = self.settings();
        if settings.theme == theme {
            return Ok(());
        }
        settings.theme = theme;
        self.repository.save(&settings)?;
        *self.settings.lock().expect("settings state lock failed") = settings;
        Ok(())
    }

    pub fn set_syntax_theme(
        &self,
        theme: lapis_settings::SyntaxTheme,
    ) -> Result<(), SettingsError> {
        let _writer = self.writer.lock().expect("settings writer lock failed");
        let mut settings = self.settings();
        if settings.syntax_theme.as_ref() == Some(&theme) {
            return Ok(());
        }
        settings.syntax_theme = Some(theme);
        self.repository.save(&settings)?;
        *self.settings.lock().expect("settings state lock failed") = settings;
        Ok(())
    }
}

/// ネイティブダイアログを利用するユースケース側の契約。
pub trait WorkspaceDialog: Send + Sync {
    /// Remoteではnative pickerを使わず、この名前空間内の相対pathを入力する。
    fn path_scope(&self) -> Option<PathBuf> {
        None
    }
    fn choose_workspace_path(&self) -> Option<PathBuf>;
    fn choose_file_path(&self) -> Option<PathBuf> {
        None
    }
    fn choose_save_path(&self, suggested_name: &str) -> Option<PathBuf>;
}

pub use lapis_workspace::WorkspaceRepository;

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
    #[serde(default)]
    pub id: Option<TerminalId>,
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
                    id: Some(terminal.id.clone()),
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
    backend: Arc<dyn lapis_git::GitConnection>,
    workspace_id: Option<WorkspaceId>,
    status: Option<lapis_git::GitSnapshot>,
    worktrees: Vec<lapis_git::WorktreeSnapshot>,
    worktree_statuses: Vec<(TaskId, lapis_git::GitSnapshot)>,
    selected_diff: Option<lapis_git::GitDiffSnapshot>,
}

impl GitSession {
    pub fn new(backend: Arc<dyn lapis_git::GitConnection>) -> Self {
        Self {
            backend,
            workspace_id: None,
            status: None,
            worktrees: Vec::new(),
            worktree_statuses: Vec::new(),
            selected_diff: None,
        }
    }

    pub fn status(&self) -> Option<&lapis_git::GitSnapshot> {
        self.status.as_ref()
    }
    pub fn worktrees(&self) -> &[lapis_git::WorktreeSnapshot] {
        &self.worktrees
    }
    pub fn selected_diff(&self) -> Option<&lapis_git::GitDiffSnapshot> {
        self.selected_diff.as_ref()
    }
    pub fn worktree_statuses(&self) -> &[(TaskId, lapis_git::GitSnapshot)] {
        &self.worktree_statuses
    }

    pub fn refresh(&mut self, workspace: &WorkspaceId) -> Result<bool, GitError> {
        let status = self.backend.status(workspace, None)?;
        let worktrees = self.backend.worktrees(workspace)?;
        let worktree_statuses = worktrees
            .iter()
            .filter(|worktree| worktree.state != lapis_git::WorktreeState::Discarded)
            .filter_map(|worktree| {
                self.backend
                    .status(workspace, Some(&worktree.worktree_id))
                    .ok()
                    .map(|status| (worktree.worktree_id.clone(), status))
            })
            .collect::<Vec<_>>();
        let changed = self.workspace_id.as_ref() != Some(workspace)
            || self.status.as_ref() != Some(&status)
            || self.worktrees != worktrees
            || self.worktree_statuses != worktree_statuses;
        if self.workspace_id.as_ref() != Some(workspace) {
            self.selected_diff = None;
        }
        self.workspace_id = Some(workspace.clone());
        self.status = Some(status);
        self.worktrees = worktrees;
        self.worktree_statuses = worktree_statuses;
        Ok(changed)
    }

    fn workspace(&self) -> Result<&WorkspaceId, GitError> {
        self.workspace_id.as_ref().ok_or_else(|| {
            GitError::new(
                lapis_git::GitErrorKind::NotRepository,
                "Workspace がありません",
            )
        })
    }

    pub fn select_diff(&mut self, path: &str) -> Result<(), GitError> {
        self.selected_diff = Some(self.backend.diff(self.workspace()?, None, path)?);
        Ok(())
    }

    pub fn select_worktree_diff(&mut self, worktree: &TaskId, path: &str) -> Result<(), GitError> {
        self.selected_diff = Some(self.backend.diff(self.workspace()?, Some(worktree), path)?);
        Ok(())
    }

    pub fn import_file(&mut self, worktree: &TaskId, path: &str) -> Result<(), GitError> {
        let workspace = self.workspace()?.clone();
        self.backend.import_file(&workspace, worktree, path)?;
        self.refresh(&workspace)?;
        Ok(())
    }

    pub fn discard_worktree(&mut self, worktree: &TaskId) -> Result<(), GitError> {
        let workspace = self.workspace()?.clone();
        self.backend.discard_worktree(&workspace, worktree)?;
        self.refresh(&workspace)?;
        Ok(())
    }

    pub fn backend(&self) -> Arc<dyn lapis_git::GitConnection> {
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

    /// 保存済みidentityだけを再アタッチする。shellやCLIを再実行しない。
    pub fn restore_summaries(&mut self, terminals: &[RestoredTerminal]) {
        self.terminals = terminals
            .iter()
            .enumerate()
            .map(|(index, terminal)| TerminalSnapshot {
                id: terminal
                    .id
                    .clone()
                    .unwrap_or_else(|| TerminalId::new(format!("restored-terminal-{}", index + 1))),
                cwd: terminal.cwd.clone(),
                status: if terminal.id.is_some() {
                    TerminalStatus::Disconnected
                } else {
                    TerminalStatus::Exited
                },
                columns: terminal.columns,
                rows: terminal.rows,
                output: Vec::new(),
                output_sequence: 0,
                output_truncated: false,
                output_offset: 0,
                screen: None,
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
        self.start_with_shell(cwd, columns, rows, lapis_terminal::TerminalShell::Default)
    }

    pub fn start_with_shell(
        &mut self,
        cwd: &Path,
        columns: u16,
        rows: u16,
        shell: lapis_terminal::TerminalShell,
    ) -> Result<TerminalId, TerminalError> {
        let id = self.backend.start_with_shell(cwd, columns, rows, shell)?;
        if let Ok(snapshot) = self.backend.attach(cwd, &id) {
            self.terminals.push(snapshot);
            return Ok(id);
        }
        self.terminals.push(TerminalSnapshot {
            id: id.clone(),
            cwd: cwd.to_owned(),
            status: TerminalStatus::Running,
            columns,
            rows,
            output: Vec::new(),
            output_sequence: 0,
            output_truncated: false,
            output_offset: 0,
            screen: None,
        });
        Ok(id)
    }

    pub fn discover(&self, cwd: &Path) -> Result<Vec<TerminalSnapshot>, TerminalError> {
        self.backend.list(cwd)
    }

    pub fn attach(&mut self, cwd: &Path, id: &TerminalId) -> Result<(), TerminalError> {
        let snapshot = self.backend.attach(cwd, id)?;
        if let Some(existing) = self
            .terminals
            .iter_mut()
            .find(|terminal| terminal.id == *id)
        {
            *existing = snapshot;
        } else {
            self.terminals.push(snapshot);
        }
        Ok(())
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
        let mut attach_error = None;
        for terminal in &mut self.terminals {
            if terminal.status == TerminalStatus::Disconnected {
                match self.backend.attach(&terminal.cwd, &terminal.id) {
                    Ok(snapshot) => {
                        *terminal = snapshot;
                        changed = true;
                    }
                    Err(error) => {
                        attach_error.get_or_insert_with(|| error.to_string());
                        continue;
                    }
                }
            }
            if terminal.status != TerminalStatus::Running {
                continue;
            }
            for event in self.backend.poll(&terminal.id)? {
                match event {
                    TerminalEvent::Snapshot(mut snapshot) => {
                        if snapshot.id != terminal.id
                            || snapshot.output_sequence < terminal.output_sequence
                        {
                            continue;
                        }
                        const OUTPUT_LIMIT: usize = 256 * 1024;
                        if snapshot.output.len() > OUTPUT_LIMIT {
                            snapshot.output_offset = snapshot
                                .output_offset
                                .saturating_add((snapshot.output.len() - OUTPUT_LIMIT) as u64);
                            snapshot
                                .output
                                .drain(..snapshot.output.len() - OUTPUT_LIMIT);
                            snapshot.output_truncated = true;
                        }
                        *terminal = snapshot;
                    }
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
                            terminal.output_offset =
                                terminal.output_offset.saturating_add(start as u64);
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
            if terminal.screen.as_ref().is_some_and(|screen| {
                if screen.output_end < terminal.output_offset {
                    return true;
                }
                let suffix = (screen.output_end - terminal.output_offset) as usize;
                terminal.output.get(suffix..).is_some_and(|bytes| {
                    bytes
                        .windows(7)
                        .any(|window| window == b"\x1b[?1049" || window == b"\x1b[?1047")
                        || bytes.windows(5).any(|window| window == b"\x1b[?47")
                })
            }) {
                // buffer欠落や通常/代替画面の切替時は、Hostの現在画面を取り直す。
                match self.backend.attach(&terminal.cwd, &terminal.id) {
                    Ok(snapshot) => {
                        *terminal = snapshot;
                        changed = true;
                    }
                    Err(error) => {
                        attach_error.get_or_insert_with(|| error.to_string());
                    }
                }
            }
        }
        match attach_error {
            Some(error) => Err(TerminalError::new(error)),
            None => Ok(changed),
        }
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
    diagnostics_request: Option<(PathBuf, Revision)>,
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
            diagnostics_request: None,
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

    pub fn refresh(&mut self, editor: &EditorSession) -> Result<bool, LspError> {
        if self.started_workspace.is_none() {
            return Ok(false);
        }
        let current_document = editor
            .active_path()
            .map(|path| (path.to_owned(), editor.active_revision()));
        if let Some(receiver) = &self.diagnostics_receiver {
            if self.diagnostics_request.as_ref() != current_document.as_ref() {
                self.diagnostics_receiver = None;
                self.diagnostics_request = None;
                return Ok(false);
            }
            match receiver.try_recv() {
                Ok(result) => {
                    self.diagnostics_receiver = None;
                    let request = self.diagnostics_request.take();
                    let diagnostics = result?;
                    if let Some((path, revision)) = request
                        && diagnostics.iter().any(|diagnostic| {
                            diagnostic.path == path && diagnostic.revision != revision
                        })
                    {
                        return Ok(false);
                    }
                    let changed = diagnostics != self.diagnostics;
                    self.diagnostics = diagnostics;
                    return Ok(changed);
                }
                Err(mpsc::TryRecvError::Empty) => return Ok(false),
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.diagnostics_receiver = None;
                    self.diagnostics_request = None;
                    return Err(LspError::new("LSP diagnostics worker disconnected"));
                }
            }
        }
        let Some(request) = self.synced_document.clone() else {
            return Ok(false);
        };
        let backend = self.backend.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(backend.diagnostics());
        });
        self.diagnostics_receiver = Some(receiver);
        self.diagnostics_request = Some(request);
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
        self.diagnostics_request = None;
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

pub struct TaskSession {
    backend: Arc<dyn lapis_task::TaskConnection>,
    conversation: ConversationId,
    workspace_id: Option<WorkspaceId>,
    records: Vec<lapis_task::TaskSnapshot>,
    restore_warning: Option<String>,
}

impl TaskSession {
    pub fn new(backend: Arc<dyn lapis_task::TaskConnection>) -> Self {
        Self {
            backend,
            conversation: ConversationId::new("conversation-local"),
            workspace_id: None,
            records: Vec::new(),
            restore_warning: None,
        }
    }

    pub fn records(&self) -> &[lapis_task::TaskSnapshot] {
        &self.records
    }
    pub fn restore_warning(&self) -> Option<&str> {
        self.restore_warning.as_deref()
    }

    pub fn refresh(&mut self, workspace: &WorkspaceId) -> Result<bool, TaskError> {
        let records = self.backend.load(workspace)?;
        let changed = self.workspace_id.as_ref() != Some(workspace) || self.records != records;
        self.workspace_id = Some(workspace.clone());
        self.records = records;
        Ok(changed)
    }

    pub fn start_codex(
        &mut self,
        workspace: WorkspaceId,
        prompt: impl Into<String>,
    ) -> Result<ExecutionId, TaskError> {
        self.start_codex_with_mode(workspace, prompt, TaskMode::Default)
    }

    pub fn start_codex_with_mode(
        &mut self,
        workspace: WorkspaceId,
        prompt: impl Into<String>,
        mode: TaskMode,
    ) -> Result<ExecutionId, TaskError> {
        self.start(workspace, prompt.into(), mode, false)
    }

    pub fn start_codex_in_worktree(
        &mut self,
        workspace: WorkspaceId,
        prompt: impl Into<String>,
        mode: TaskMode,
    ) -> Result<ExecutionId, TaskError> {
        self.start(workspace, prompt.into(), mode, true)
    }

    fn start(
        &mut self,
        workspace: WorkspaceId,
        prompt: String,
        mode: TaskMode,
        isolated: bool,
    ) -> Result<ExecutionId, TaskError> {
        let record = self.backend.start(
            &workspace,
            lapis_task::TaskStart {
                prompt,
                mode,
                isolated,
                conversation_id: self.conversation.clone(),
            },
        )?;
        let execution_id = record.execution.id.clone();
        if self.workspace_id.as_ref() != Some(&workspace) {
            self.records.clear();
        }
        self.workspace_id = Some(workspace);
        self.records.insert(0, record);
        Ok(execution_id)
    }

    pub fn control(
        &mut self,
        execution_id: &ExecutionId,
        control: TaskControl,
    ) -> Result<(), TaskError> {
        let record = self
            .records
            .iter()
            .find(|record| &record.execution.id == execution_id)
            .ok_or_else(|| TaskError::new("Task Execution が見つかりません"))?;
        self.backend
            .control(&record.execution.workspace_id, execution_id, &control)
    }

    pub fn backend(&self) -> Arc<dyn lapis_task::TaskConnection> {
        self.backend.clone()
    }

    pub fn set_conversation(&mut self, conversation: ConversationId) {
        self.conversation = conversation;
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
    can_undo: bool,
    can_redo: bool,
}

impl OpenDocument {
    fn from_cache(cache: DocumentCache, view: DocumentViewState) -> Self {
        Self {
            id: cache.id,
            document: cache.document,
            external_change: cache.external_change,
            can_undo: cache.can_undo,
            can_redo: cache.can_redo,
            view,
        }
    }

    fn update(&mut self, cache: DocumentCache) {
        self.document = cache.document;
        self.external_change = cache.external_change;
        self.can_undo = cache.can_undo;
        self.can_redo = cache.can_redo;
    }
}

/// UIからDocumentと外部I/Oを分離する、複数文書Workspace編集セッション。
pub struct EditorSession {
    documents: Vec<OpenDocument>,
    active: Option<usize>,
    workspace_root: Option<PathBuf>,
    workspace_restore_root: Option<PathBuf>,
    workspace_id: Option<WorkspaceId>,
    workspace_name: String,
    file_tree: Vec<FileEntry>,
    document_connection: Arc<dyn DocumentConnection>,
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
        document_connection: Arc<dyn DocumentConnection>,
    ) -> Self {
        let mut session = Self::new_empty(
            repository,
            file_dialog,
            state_repository,
            document_connection,
        );
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
        document_connection: Arc<dyn DocumentConnection>,
    ) -> Self {
        Self {
            documents: Vec::new(),
            active: None,
            workspace_root: None,
            workspace_restore_root: None,
            workspace_id: None,
            workspace_name: "No Workspace".to_owned(),
            file_tree: Vec::new(),
            document_connection: document_connection.fork(),
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

    pub fn workspace_path(
        &self,
        expected_root: &Path,
        input: &str,
    ) -> Result<PathBuf, WorkspaceError> {
        if self.workspace_root() != Some(expected_root) {
            return Err(WorkspaceError::new(
                "作業環境が切り替わりました。操作をやり直してください",
            ));
        }
        if input.is_empty()
            || Path::new(input)
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err(WorkspaceError::new(
                "Workspace内の相対pathを指定してください",
            ));
        }
        Ok(expected_root.join(input))
    }

    pub fn workspace_id(&self) -> Option<&WorkspaceId> {
        self.workspace_id.as_ref()
    }

    pub fn workspace_name(&self) -> &str {
        &self.workspace_name
    }

    pub fn snapshot(&self) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            root: self
                .workspace_root
                .clone()
                .or_else(|| self.workspace_restore_root.clone()),
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
            active_document_id: self.active_document_id().cloned(),
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
        let open = self.required_document()?;
        if range.is_empty() && replacement.is_empty() {
            return Ok(false);
        }
        let old_revision = open.document.revision();
        let result = self
            .document_connection
            .edit(&open.id, old_revision, range, replacement);
        let cache = self.refresh_after_error(result)?;
        let changed = cache.document.revision() != old_revision;
        self.required_document_mut()?.update(cache);
        if changed {
            self.persist_best_effort();
        }
        Ok(changed)
    }

    pub fn undo(&mut self) -> bool {
        self.change_history(true)
    }

    pub fn redo(&mut self) -> bool {
        self.change_history(false)
    }

    pub fn can_undo(&self) -> bool {
        self.active_document().is_some_and(|open| open.can_undo)
    }

    pub fn can_redo(&self) -> bool {
        self.active_document().is_some_and(|open| open.can_redo)
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
        let cache = match self.document_connection.create(self.workspace_root()) {
            Ok(cache) => cache,
            Err(error) => {
                self.restore_warning = Some(error.to_string());
                return;
            }
        };
        let id = cache.id.clone();
        let view = DocumentViewState {
            document_id: Some(id.clone()),
            ..DocumentViewState::default()
        };
        self.documents.push(OpenDocument::from_cache(cache, view));
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
        let workspace_id = self
            .document_connection
            .workspace(&root)
            .map_err(|error| WorkspaceError::new(error.to_string()))?;
        let tree = self.repository.list_tree(&root)?;
        self.detach_documents();
        self.workspace_name = root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Workspace")
            .to_owned();
        self.workspace_root = Some(root);
        self.workspace_restore_root = None;
        self.workspace_id = Some(workspace_id);
        self.file_tree = tree;
        self.documents.clear();
        self.active = None;
        self.persist()?;
        Ok(())
    }

    pub fn close_workspace(&mut self) -> Result<(), WorkspaceError> {
        self.detach_documents();
        self.workspace_root = None;
        self.workspace_restore_root = None;
        self.workspace_id = None;
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

    pub fn document_connection(&self) -> Arc<dyn DocumentConnection> {
        self.document_connection.clone()
    }

    pub fn file_dialog(&self) -> Arc<dyn WorkspaceDialog> {
        self.file_dialog.clone()
    }

    pub fn state_repository(&self) -> Arc<dyn WorkspaceStateRepository> {
        self.state_repository.clone()
    }

    pub fn new_folder(&mut self) -> Result<(), WorkspaceError> {
        let root = self
            .workspace_root()
            .ok_or_else(|| WorkspaceError::new("Workspace を開いてください"))?;
        let timestamp = lapis_task::unix_time_ms();
        self.repository
            .create_directory(root, &format!("new-folder-{timestamp}"))?;
        self.refresh_file_tree()
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
        let cache = self.document_connection.open(&path)?;
        if let Some(index) = self.documents.iter().position(|open| open.id == cache.id) {
            self.documents[index].update(cache);
            self.active = Some(index);
            self.persist_best_effort();
            return Ok(());
        }
        let id = cache.id.clone();
        self.documents.push(OpenDocument::from_cache(
            cache,
            DocumentViewState {
                document_id: Some(id),
                path,
                ..DocumentViewState::default()
            },
        ));
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
        if !self.select_document_for_operation(id) {
            return false;
        }
        self.persist_best_effort();
        true
    }

    /// Worker の操作対象だけを切り替える。表示状態の保存や文書 I/O は行わない。
    /// ユーザーによるタブ選択の保存には `activate_document` を使う。
    pub fn select_document_for_operation(&mut self, id: &DocumentId) -> bool {
        if self.active_document_id() == Some(id) {
            return true;
        }
        let Some(index) = self.documents.iter().position(|open| &open.id == id) else {
            return false;
        };
        self.active = Some(index);
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
        if disposition == DocumentCloseDisposition::DiscardChanges {
            match self
                .document_connection
                .discard(id, self.documents[index].document.revision())
            {
                Ok(cache) => self.documents[index].update(cache),
                Err(error) => {
                    if let Ok(cache) = self.document_connection.attach(id) {
                        self.documents[index].update(cache);
                        self.persist_best_effort();
                    }
                    return Err(error);
                }
            }
        }
        self.document_connection.close(id)?;
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

    /// Validates all close decisions, then closes the documents in order.
    ///
    /// Each tuple contains a document ID, its approved disposition, and the revision
    /// confirmed by the caller. Complete any required approval and saving first.
    /// Validation errors leave all tabs open. Discard and close errors propagate;
    /// documents already closed before such an error remain closed.
    pub fn close_documents(
        &mut self,
        documents: &[(
            DocumentId,
            DocumentCloseDisposition,
            lapis_document::Revision,
        )],
    ) -> Result<(), DocumentError> {
        self.validate_document_closes(documents)?;
        for (id, disposition, _) in documents {
            self.close_document(id, *disposition)?;
        }
        Ok(())
    }

    /// Checks approved revisions and refreshes matching open-document caches without closing tabs.
    ///
    /// Each tuple supplies a document ID, approved disposition, and expected revision.
    /// Returns a conflict if a revision changed or a dirty document would be closed
    /// with `PreserveChanges`. Attachment errors propagate. Caches refreshed before
    /// an error remain updated; this method does not save or discard changes.
    pub fn validate_document_closes(
        &mut self,
        documents: &[(
            DocumentId,
            DocumentCloseDisposition,
            lapis_document::Revision,
        )],
    ) -> Result<(), DocumentError> {
        for (id, disposition, revision) in documents {
            let current = self.document_connection.attach(id)?;
            if current.document.revision() != *revision {
                return Err(DocumentError::conflict("確認後に文書が変更されました"));
            }
            if current.document.is_dirty()
                && *disposition == DocumentCloseDisposition::PreserveChanges
            {
                return Err(DocumentError::conflict("未保存の文書を保持します"));
            }
            if let Some(open) = self.documents.iter_mut().find(|open| &open.id == id) {
                open.update(current);
            }
        }
        Ok(())
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

        let open = self.required_document()?;
        let target = if open.document.path().is_none() {
            Some(path)
        } else {
            None
        };
        let result = self
            .document_connection
            .save(&open.id, open.document.revision(), target);
        let cache = self.refresh_after_error(result)?;
        self.required_document_mut()?.update(cache);
        self.persist_best_effort();
        Ok(DocumentAction::Completed)
    }

    pub fn save_document_to(&mut self, path: PathBuf) -> Result<(), DocumentError> {
        let open = self.required_document()?;
        let result = self
            .document_connection
            .save(&open.id, open.document.revision(), Some(path));
        let cache = self.refresh_after_error(result)?;
        self.required_document_mut()?.update(cache);
        self.persist_best_effort();
        Ok(())
    }

    pub fn save_document_as(&mut self) -> Result<DocumentAction, DocumentError> {
        let Some(path) = self.file_dialog.choose_save_path(&self.display_name()) else {
            return Ok(DocumentAction::Cancelled);
        };
        self.save_document_to(path)?;
        Ok(DocumentAction::Completed)
    }

    pub fn reload_active_from_disk(&mut self) -> Result<(), DocumentError> {
        let open = self.required_document()?;
        let result = self
            .document_connection
            .reload(&open.id, open.document.revision());
        let cache = self.refresh_after_error(result)?;
        self.required_document_mut()?.update(cache);
        self.persist_best_effort();
        Ok(())
    }

    pub fn poll_external_changes(&mut self) -> Vec<(DocumentId, ExternalChange)> {
        let mut changes = Vec::new();
        for open in &mut self.documents {
            let cache = match self.document_connection.attach(&open.id) {
                Ok(value) => value,
                Err(_) => continue,
            };
            if cache.external_change != open.external_change
                || cache.document != open.document
                || cache.can_undo != open.can_undo
                || cache.can_redo != open.can_redo
            {
                changes.push((open.id.clone(), cache.external_change));
            }
            open.update(cache);
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

    fn change_history(&mut self, undo: bool) -> bool {
        let Some(open) = self.active_document() else {
            return false;
        };
        let revision = open.document.revision();
        let result = self.document_connection.history(&open.id, revision, undo);
        match self.refresh_after_error(result) {
            Ok(cache) => {
                let changed = cache.document.revision() != revision;
                if let Some(open) = self.active_document_mut() {
                    open.update(cache);
                }
                self.persist_best_effort();
                changed
            }
            Err(error) => {
                self.restore_warning = Some(error.to_string());
                false
            }
        }
    }

    fn refresh_after_error(
        &mut self,
        result: Result<DocumentCache, DocumentError>,
    ) -> Result<DocumentCache, DocumentError> {
        if result.is_err()
            && let Some(open) = self.active_document()
            && let Ok(cache) = self.document_connection.attach(&open.id)
            && let Some(open) = self.active_document_mut()
        {
            open.update(cache);
        }
        result
    }

    fn restore(&mut self) -> Result<(), WorkspaceError> {
        let Some(snapshot) = self.state_repository.load()? else {
            return Ok(());
        };
        self.restore_snapshot(snapshot)
    }

    fn recover_document_draft(&self, draft: &str) -> Result<DocumentCache, WorkspaceError> {
        let cache = self
            .document_connection
            .create(self.workspace_root())
            .map_err(|error| WorkspaceError::new(error.to_string()))?;
        self.document_connection
            .edit(
                &cache.id,
                cache.document.revision(),
                0..cache.document.len_chars(),
                draft,
            )
            .map_err(|error| WorkspaceError::new(error.to_string()))
    }

    pub fn recover_draft(&mut self, draft: &str) -> Result<(), DocumentError> {
        let cache = self
            .recover_document_draft(draft)
            .map_err(|error| DocumentError::io(error.to_string()))?;
        let id = cache.id.clone();
        self.documents.push(OpenDocument::from_cache(
            cache,
            DocumentViewState {
                document_id: Some(id),
                ..Default::default()
            },
        ));
        self.active = Some(self.documents.len() - 1);
        // 文書復旧は成功している。表示状態の保存失敗は警告として保持する。
        self.persist_best_effort();
        Ok(())
    }

    pub fn restore_snapshot(&mut self, snapshot: WorkspaceSnapshot) -> Result<(), WorkspaceError> {
        self.workspace_restore_root = snapshot.root.clone();
        self.detach_documents();
        self.documents.clear();
        self.active = None;
        self.workspace_root = None;
        self.workspace_id = None;
        self.file_tree.clear();
        self.workspace_name = "No Workspace".to_owned();
        if let Some(root) = snapshot.root {
            let workspace = self
                .document_connection
                .workspace(&root)
                .map_err(|error| WorkspaceError::new(error.to_string()))
                .and_then(|id| self.repository.list_tree(&root).map(|tree| (id, tree)));
            match workspace {
                Ok((id, tree)) => {
                    self.workspace_id = Some(id);
                    self.workspace_name = root
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("Workspace")
                        .to_owned();
                    self.file_tree = tree;
                    self.workspace_root = Some(root);
                }
                Err(error) => {
                    self.restore_warning =
                        Some(format!("Workspace を復元できませんでした: {error}"));
                }
            }
        }
        let mut restored_active = None;
        for view in snapshot.open_documents {
            let was_active = snapshot.active_document_id.is_some()
                && view.document_id == snapshot.active_document_id;
            let attached = view
                .document_id
                .as_ref()
                .and_then(|id| self.document_connection.attach(id).ok());
            let recovered = attached.is_some();
            let cache = attached.map(Ok).unwrap_or_else(|| {
                if view.path.as_os_str().is_empty() {
                    self.document_connection.create(self.workspace_root())
                } else {
                    self.document_connection.open(&view.path)
                }
            });
            let mut cache = match cache {
                Ok(cache) => cache,
                Err(error) => {
                    let Some(draft) = &view.draft_content else {
                        continue;
                    };
                    let cache = self.recover_document_draft(draft)?;
                    self.restore_warning = Some(format!(
                        "元の文書を開けないため復元 draft を未保存文書として開きました: {error}"
                    ));
                    cache
                }
            };
            if !recovered
                && let Some(draft) = &view.draft_content
                && cache.document.content_for_save() != *draft
            {
                if cache.reused {
                    self.documents
                        .push(OpenDocument::from_cache(cache, view.clone()));
                    cache = self
                        .document_connection
                        .create(self.workspace_root())
                        .map_err(|error| WorkspaceError::new(error.to_string()))?;
                    self.restore_warning = Some(
                        "復元 draft は共有文書を上書きせず未保存文書として開きました".to_owned(),
                    );
                }
                match self.document_connection.edit(
                    &cache.id,
                    cache.document.revision(),
                    0..cache.document.len_chars(),
                    draft,
                ) {
                    Ok(updated) => cache = updated,
                    Err(error) => {
                        let current = self.document_connection.attach(&cache.id).ok();
                        cache = self.recover_document_draft(draft)?;
                        if let Some(current) = current {
                            self.documents
                                .push(OpenDocument::from_cache(current, view.clone()));
                        }
                        self.restore_warning = Some(format!(
                            "復元 draft の適用に失敗したため未保存文書として開きました: {error}"
                        ));
                    }
                }
            }
            let id = cache.id.clone();
            if was_active {
                restored_active = Some(self.documents.len());
            }
            self.documents.push(OpenDocument::from_cache(
                cache,
                DocumentViewState {
                    document_id: Some(id),
                    ..view
                },
            ));
        }
        self.active = restored_active.or_else(|| {
            snapshot.active_path.and_then(|path| {
                self.documents
                    .iter()
                    .position(|open| open.document.path() == Some(path.as_path()))
            })
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

    fn detach_documents(&mut self) {
        for open in &self.documents {
            if let Err(error) = self.document_connection.close(&open.id) {
                self.restore_warning = Some(error.to_string());
            }
        }
    }

    fn persist(&self) -> Result<(), WorkspaceError> {
        self.state_repository.save(&self.snapshot())
    }
}

#[cfg(test)]
mod tests {
    include!("document_tests.rs");
    use std::{collections::HashMap, sync::Mutex};

    use lapis_document::{DocumentErrorKind, DocumentRepository, FileData, FileFingerprint};
    use lapis_lsp::{DiagnosticSeverity, LspRange};
    use lapis_task::{
        ExecutionSnapshot, ExecutionStatus, Task, TaskSnapshot, TaskStart, unix_time_ms,
    };
    use lapis_workspace::FileEntryKind;

    use super::*;

    #[test]
    fn settings_read_cache_does_not_wait_for_persistence_io() {
        struct SlowSettings {
            started: mpsc::Sender<()>,
            release: Mutex<mpsc::Receiver<()>>,
        }
        impl GlobalSettingsRepository for SlowSettings {
            fn load(&self) -> Result<GlobalSettings, SettingsError> {
                Ok(GlobalSettings::default())
            }
            fn save(&self, _: &GlobalSettings) -> Result<(), SettingsError> {
                self.started.send(()).unwrap();
                self.release.lock().unwrap().recv().unwrap();
                Ok(())
            }
        }
        let (started, receiver) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let session = SettingsSession::load(Arc::new(SlowSettings {
            started,
            release: Mutex::new(released),
        }))
        .unwrap();
        let writer_session = session.clone();
        let writer = thread::spawn(move || writer_session.set_theme("new-theme".into()));
        receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        let (ready, read) = mpsc::channel();
        let reader_session = session.clone();
        let reader = thread::spawn(move || ready.send(reader_session.settings()).unwrap());
        let cached = read.recv_timeout(std::time::Duration::from_secs(2));
        release.send(()).unwrap();
        writer.join().unwrap().unwrap();
        reader.join().unwrap();
        assert_eq!(cached.unwrap(), GlobalSettings::default());
        assert_eq!(session.settings().theme, "new-theme");
    }

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
        fn create_directory(&self, _: &Path, _: &str) -> Result<(), WorkspaceError> {
            Err(WorkspaceError::new("Unsupported"))
        }
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
    struct MemoryState {
        snapshot: Mutex<Option<WorkspaceSnapshot>>,
        saves: std::sync::atomic::AtomicUsize,
    }

    impl WorkspaceStateRepository for MemoryState {
        fn load(&self) -> Result<Option<WorkspaceSnapshot>, WorkspaceError> {
            Ok(self.snapshot.lock().unwrap().clone())
        }

        fn save(&self, snapshot: &WorkspaceSnapshot) -> Result<(), WorkspaceError> {
            self.saves
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            *self.snapshot.lock().unwrap() = Some(snapshot.clone());
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
        records: Mutex<Vec<TaskSnapshot>>,
        controls: Mutex<Vec<(ExecutionId, TaskControl)>>,
        starts: Mutex<Vec<TaskStart>>,
    }

    struct UnavailableTerminalBackend;

    struct ControlledLspBackend {
        diagnostics_started: Mutex<mpsc::Sender<()>>,
        diagnostics_finished: Mutex<mpsc::Sender<()>>,
        diagnostics_results: Mutex<mpsc::Receiver<Result<Vec<Diagnostic>, LspError>>>,
    }

    impl LanguageServerBackend for ControlledLspBackend {
        fn start(&self, _workspace: &Path) -> Result<(), LspError> {
            Ok(())
        }

        fn did_open(&self, _path: &Path, _text: &str, _revision: Revision) -> Result<(), LspError> {
            Ok(())
        }

        fn did_change(
            &self,
            _path: &Path,
            _text: &str,
            _revision: Revision,
        ) -> Result<(), LspError> {
            Ok(())
        }

        fn diagnostics(&self) -> Result<Vec<Diagnostic>, LspError> {
            self.diagnostics_started.lock().unwrap().send(()).unwrap();
            let result = self.diagnostics_results.lock().unwrap().recv().unwrap();
            self.diagnostics_finished.lock().unwrap().send(()).unwrap();
            result
        }

        fn completion(
            &self,
            _path: &Path,
            _position: LspPosition,
            _revision: Revision,
        ) -> Result<Vec<CompletionItem>, LspError> {
            Ok(Vec::new())
        }

        fn definition(
            &self,
            _path: &Path,
            _position: LspPosition,
            _revision: Revision,
        ) -> Result<Option<DefinitionTarget>, LspError> {
            Ok(None)
        }

        fn shutdown(&self) -> Result<(), LspError> {
            Ok(())
        }
    }

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

    struct SelectiveAttachBackend;

    impl TerminalBackend for SelectiveAttachBackend {
        fn start(
            &self,
            _cwd: &Path,
            _columns: u16,
            _rows: u16,
        ) -> Result<TerminalId, TerminalError> {
            panic!("refresh must never start a terminal")
        }

        fn input(&self, _id: &TerminalId, _bytes: &[u8]) -> Result<(), TerminalError> {
            unreachable!("refresh does not send terminal input")
        }

        fn resize(&self, _id: &TerminalId, _columns: u16, _rows: u16) -> Result<(), TerminalError> {
            unreachable!("refresh does not resize terminals")
        }

        fn poll(&self, id: &TerminalId) -> Result<Vec<TerminalEvent>, TerminalError> {
            if id.as_str() == "recoverable" {
                Ok(vec![TerminalEvent::Output(
                    lapis_terminal::TerminalOutput {
                        sequence: 1,
                        data: b"later terminal".to_vec(),
                    },
                )])
            } else {
                Ok(Vec::new())
            }
        }

        fn terminate(&self, _id: &TerminalId) -> Result<(), TerminalError> {
            unreachable!("refresh does not terminate terminals")
        }

        fn attach(&self, cwd: &Path, id: &TerminalId) -> Result<TerminalSnapshot, TerminalError> {
            if id.as_str() == "unavailable" {
                return Err(TerminalError::new("terminal is still unavailable"));
            }
            Ok(TerminalSnapshot {
                id: id.clone(),
                cwd: cwd.to_owned(),
                status: TerminalStatus::Running,
                columns: 80,
                rows: 24,
                output: Vec::new(),
                output_sequence: 0,
                output_truncated: false,
                output_offset: 0,
                screen: None,
            })
        }
    }

    impl lapis_task::TaskConnection for MemoryTaskBackend {
        fn load(&self, workspace: &WorkspaceId) -> Result<Vec<TaskSnapshot>, TaskError> {
            Ok(self
                .records
                .lock()
                .unwrap()
                .iter()
                .filter(|record| &record.execution.workspace_id == workspace)
                .cloned()
                .collect())
        }

        fn start(
            &self,
            workspace: &WorkspaceId,
            input: TaskStart,
        ) -> Result<TaskSnapshot, TaskError> {
            let mut records = self.records.lock().unwrap();
            let sequence = records.len() + 1;
            let task_id = TaskId::new(format!("canonical-task-{sequence}"));
            let record = TaskSnapshot {
                task: Task {
                    id: task_id.clone(),
                    conversation_id: input.conversation_id.clone(),
                    title: input.prompt.clone(),
                    prompt: input.prompt.clone(),
                    created_at_ms: unix_time_ms(),
                },
                execution: ExecutionSnapshot {
                    id: ExecutionId::new(format!("canonical-execution-{sequence}")),
                    task_id: task_id.clone(),
                    workspace_id: workspace.clone(),
                    worktree_id: input.isolated.then_some(task_id),
                    runner: "test".to_owned(),
                    mode: input.mode,
                    status: ExecutionStatus::Queued,
                    started_at_ms: None,
                    completed_at_ms: None,
                    external_thread_id: None,
                    failure: None,
                },
                events: Vec::new(),
            };
            self.starts.lock().unwrap().push(input);
            records.insert(0, record.clone());
            Ok(record)
        }

        fn control(
            &self,
            _workspace: &WorkspaceId,
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
        assert!(
            settings
                .set_syntax_theme(lapis_settings::SyntaxTheme::Zed)
                .is_err()
        );
        assert_eq!(settings.settings().syntax_theme, None);
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
        let documents = Arc::new(MemoryDocuments::new(repository.clone()));
        EditorSession::new(
            repository,
            Arc::new(FixedDialog {
                workspace: None,
                save: None,
            }),
            state,
            documents,
        )
    }
    #[test]
    fn new_empty_starts_without_workspace_or_document() {
        let repository = Arc::new(MemoryRepository::default());
        let editor = EditorSession::new_empty(
            repository.clone(),
            Arc::new(FixedDialog {
                workspace: None,
                save: None,
            }),
            Arc::new(MemoryState::default()),
            Arc::new(MemoryDocuments::new(repository)),
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
    fn cancelled_save_path_keeps_all_tabs_and_the_unsaved_input() {
        let mut editor = session(
            Arc::new(MemoryRepository::default()),
            Arc::new(MemoryState::default()),
        );
        let first = editor.active_document_id().unwrap().clone();
        editor.new_document();
        editor.replace_range(0..0, "keep draft").unwrap();
        let dirty = editor.active_document_id().unwrap().clone();
        assert_eq!(editor.save_document().unwrap(), DocumentAction::Cancelled);
        assert_eq!(editor.tabs().len(), 2);
        assert!(editor.tabs().iter().any(|tab| tab.id == first));
        assert!(editor.tabs().iter().any(|tab| tab.id == dirty && tab.dirty));
        assert_eq!(
            editor
                .document_connection()
                .attach(&dirty)
                .unwrap()
                .document
                .content_for_save(),
            "keep draft"
        );
    }

    #[test]
    fn group_close_preflight_keeps_clean_tabs_when_another_tab_is_unapproved() {
        let repository = Arc::new(MemoryRepository::default());
        let mut editor = session(repository, Arc::new(MemoryState::default()));
        let clean = editor.active_document_id().unwrap().clone();
        let clean_revision = editor.active_revision();
        editor.new_document();
        editor.replace_range(0..0, "draft").unwrap();
        let dirty = editor.active_document_id().unwrap().clone();
        let dirty_revision = editor.active_revision();
        let plan = vec![
            (
                clean.clone(),
                DocumentCloseDisposition::PreserveChanges,
                clean_revision,
            ),
            (
                dirty.clone(),
                DocumentCloseDisposition::PreserveChanges,
                dirty_revision,
            ),
        ];
        assert!(editor.close_documents(&plan).is_err());
        assert!(editor.tabs().iter().any(|tab| tab.id == clean));
        assert!(editor.tabs().iter().any(|tab| tab.id == dirty && tab.dirty));
        editor
            .close_documents(&[
                (
                    clean,
                    DocumentCloseDisposition::PreserveChanges,
                    clean_revision,
                ),
                (
                    dirty,
                    DocumentCloseDisposition::DiscardChanges,
                    dirty_revision,
                ),
            ])
            .unwrap();
        assert!(editor.tabs().is_empty());
    }

    #[test]
    fn group_close_rejects_changes_after_confirmation_without_closing_any_tab() {
        let repository = Arc::new(MemoryRepository::default());
        let mut editor = session(repository, Arc::new(MemoryState::default()));
        let first = editor.active_document_id().unwrap().clone();
        let first_revision = editor.active_revision();
        editor.new_document();
        let second = editor.active_document_id().unwrap().clone();
        let confirmed_revision = editor.active_revision();
        editor.replace_range(0..0, "new input").unwrap();
        assert!(
            editor
                .close_documents(&[
                    (
                        first.clone(),
                        DocumentCloseDisposition::PreserveChanges,
                        first_revision
                    ),
                    (
                        second.clone(),
                        DocumentCloseDisposition::DiscardChanges,
                        confirmed_revision
                    ),
                ])
                .is_err()
        );
        assert_eq!(editor.tabs().len(), 2);
        assert!(editor.tabs().iter().any(|tab| tab.id == first));
        assert!(
            editor
                .tabs()
                .iter()
                .any(|tab| tab.id == second && tab.dirty)
        );
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
            id: None,
            cwd: PathBuf::from("repo"),
            status: TerminalStatus::Running,
            columns: 120,
            rows: 30,
        }]);

        assert_eq!(terminals.terminals().len(), 1);
        assert_eq!(terminals.terminals()[0].status, TerminalStatus::Exited);
        assert!(terminals.terminals()[0].output.is_empty());
        assert!(!terminals.refresh().unwrap());

        terminals.restore_summaries(&[RestoredTerminal {
            id: Some(TerminalId::new("live-host-terminal")),
            cwd: PathBuf::from("repo"),
            status: TerminalStatus::Running,
            columns: 80,
            rows: 24,
        }]);
        assert_eq!(
            terminals.terminals()[0].status,
            TerminalStatus::Disconnected
        );
        assert!(terminals.refresh().is_err());
        assert_eq!(
            terminals.terminals()[0].status,
            TerminalStatus::Disconnected
        );
    }

    #[test]
    fn terminal_refresh_continues_after_one_attach_failure() {
        let mut terminals = TerminalSession::new(Arc::new(SelectiveAttachBackend));
        terminals.restore_summaries(&[
            RestoredTerminal {
                id: Some(TerminalId::new("unavailable")),
                cwd: PathBuf::from("first"),
                status: TerminalStatus::Running,
                columns: 80,
                rows: 24,
            },
            RestoredTerminal {
                id: Some(TerminalId::new("recoverable")),
                cwd: PathBuf::from("second"),
                status: TerminalStatus::Running,
                columns: 80,
                rows: 24,
            },
        ]);

        let error = terminals
            .refresh()
            .expect_err("the failed attach is reported");
        assert_eq!(error.to_string(), "terminal is still unavailable");
        assert_eq!(
            terminals.terminals()[0].status,
            TerminalStatus::Disconnected
        );
        assert_eq!(terminals.terminals()[1].status, TerminalStatus::Running);
        assert_eq!(terminals.terminals()[1].output, b"later terminal");
    }

    #[test]
    fn saved_terminal_identity_reattaches_without_starting_a_shell() {
        struct AttachOnly;
        impl TerminalBackend for AttachOnly {
            fn start(&self, _: &Path, _: u16, _: u16) -> Result<TerminalId, TerminalError> {
                panic!("restore must never start a process")
            }
            fn input(&self, _: &TerminalId, _: &[u8]) -> Result<(), TerminalError> {
                unreachable!()
            }
            fn resize(&self, _: &TerminalId, _: u16, _: u16) -> Result<(), TerminalError> {
                unreachable!()
            }
            fn poll(&self, _: &TerminalId) -> Result<Vec<TerminalEvent>, TerminalError> {
                Ok(Vec::new())
            }
            fn terminate(&self, _: &TerminalId) -> Result<(), TerminalError> {
                unreachable!()
            }
            fn attach(
                &self,
                cwd: &Path,
                id: &TerminalId,
            ) -> Result<TerminalSnapshot, TerminalError> {
                assert_eq!(id.as_str(), "host-terminal");
                Ok(TerminalSnapshot {
                    id: id.clone(),
                    cwd: cwd.to_owned(),
                    status: TerminalStatus::Running,
                    columns: 100,
                    rows: 28,
                    output: b"already running".to_vec(),
                    output_sequence: 7,
                    output_truncated: false,
                    output_offset: 0,
                    screen: None,
                })
            }
        }
        let mut session = TerminalSession::new(Arc::new(AttachOnly));
        session.restore_summaries(&[RestoredTerminal {
            id: Some(TerminalId::new("host-terminal")),
            cwd: "repo".into(),
            status: TerminalStatus::Running,
            columns: 80,
            rows: 24,
        }]);
        assert_eq!(session.terminals()[0].status, TerminalStatus::Disconnected);
        session.refresh().unwrap();
        assert_eq!(session.terminals()[0].id.as_str(), "host-terminal");
        assert_eq!(session.terminals()[0].output, b"already running");
        assert_eq!(session.terminals()[0].output_sequence, 7);
        assert_eq!(session.terminals()[0].status, TerminalStatus::Running);
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
        let workspace = WorkspaceId::new("canonical-workspace");
        let backend = Arc::new(MemoryTaskBackend::default());
        let mut tasks = TaskSession::new(backend.clone());
        let conversation = ConversationId::new("active-conversation");
        tasks.set_conversation(conversation.clone());
        let execution_id = tasks
            .start_codex_with_mode(workspace.clone(), "質問する", TaskMode::Plan)
            .unwrap();

        assert_eq!(tasks.records().len(), 1);
        assert_eq!(tasks.records()[0].execution.mode, TaskMode::Plan);
        assert_eq!(tasks.records()[0].execution.status, ExecutionStatus::Queued);
        assert_eq!(tasks.records()[0].task.conversation_id, conversation);
        tasks.set_conversation(ConversationId::new("another-conversation"));
        assert_eq!(tasks.records()[0].task.conversation_id, conversation);

        backend.records.lock().unwrap()[0].execution.status = ExecutionStatus::WaitingForInput;
        assert!(tasks.refresh(&workspace).unwrap());
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

    #[test]
    fn lsp_diagnostics_discard_results_for_an_older_document_revision() {
        let repository = Arc::new(MemoryRepository::default());
        let root = PathBuf::from("repo");
        let path = root.join("main.rs");
        insert_file(&repository, path.clone(), "fn main() {}\n");
        let mut editor = session(repository, Arc::new(MemoryState::default()));
        editor.open_workspace(root).unwrap();
        editor.open_path(path.clone()).unwrap();

        let (started_sender, started_receiver) = mpsc::channel();
        let (finished_sender, finished_receiver) = mpsc::channel();
        let (result_sender, result_receiver) = mpsc::channel();
        let backend = Arc::new(ControlledLspBackend {
            diagnostics_started: Mutex::new(started_sender),
            diagnostics_finished: Mutex::new(finished_sender),
            diagnostics_results: Mutex::new(result_receiver),
        });
        let mut lsp = LspSession::new(backend);
        lsp.sync_active(&editor).unwrap();
        assert!(!lsp.refresh(&editor).unwrap());
        started_receiver.recv().unwrap();

        let old_revision = editor.active_revision();
        editor.replace_range(0..0, "// changed\n").unwrap();
        result_sender
            .send(Ok(vec![Diagnostic {
                path: path.clone(),
                range: LspRange {
                    start: LspPosition {
                        line: 0,
                        utf16_column: 0,
                    },
                    end: LspPosition {
                        line: 0,
                        utf16_column: 2,
                    },
                },
                severity: DiagnosticSeverity::Warning,
                message: "stale".to_owned(),
                revision: old_revision,
            }]))
            .unwrap();

        finished_receiver.recv().unwrap();
        assert!(!lsp.refresh(&editor).unwrap());
        assert!(lsp.diagnostics().is_empty());

        lsp.sync_active(&editor).unwrap();
        assert!(!lsp.refresh(&editor).unwrap());
        started_receiver.recv().unwrap();
        result_sender
            .send(Ok(vec![Diagnostic {
                path,
                range: LspRange {
                    start: LspPosition {
                        line: 0,
                        utf16_column: 0,
                    },
                    end: LspPosition {
                        line: 0,
                        utf16_column: 2,
                    },
                },
                severity: DiagnosticSeverity::Warning,
                message: "current".to_owned(),
                revision: editor.active_revision(),
            }]))
            .unwrap();
        finished_receiver.recv().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let changed = loop {
            if lsp.refresh(&editor).unwrap() {
                break true;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        };
        assert!(changed);
        assert_eq!(lsp.diagnostics()[0].message, "current");
    }
}
