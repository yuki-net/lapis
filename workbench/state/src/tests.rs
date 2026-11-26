use super::*;
use crate::extensions::git::client::apply_git_status;
use document::{
    Document, DocumentCache, DocumentConnection, DocumentError, DocumentErrorKind,
    DocumentRepository, FileData, FileFingerprint, Revision,
};
use editor_core::{ConversationId, DocumentId, ExecutionId, TaskId, WorkspaceId};
use git::GitError;
use settings::{SettingsError, SettingsRepository, SettingsScope};
use std::{
    collections::HashMap,
    ops::Range,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::Ordering},
};
use task::{
    ExecutionSnapshot, ExecutionStatus, Task, TaskControl, TaskError, TaskMode, TaskSnapshot,
    TaskStart, unix_time_ms,
};
use terminal::{
    TerminalBackend, TerminalDelta, TerminalError, TerminalEvent, TerminalId, TerminalInput,
    TerminalRow, TerminalRowUpdate, TerminalScreen, TerminalSize, TerminalSnapshot, TerminalStatus,
};
use workspace::{FileEntry, FileEntryKind, WorkspaceError, WorkspaceIdentity};

include!("document_tests.rs");
include!("git_tests.rs");

#[test]
fn stale_git_events_do_not_replace_newer_revision() {
    let snapshot = |revision| git::GitSnapshot {
        workspace_id: WorkspaceId::new("workspace"),
        worktree_id: None,
        revision,
        branch: format!("branch-{revision}"),
        head: String::new(),
        files: vec![],
    };
    let mut current = None;
    assert!(apply_git_status(&mut current, snapshot(15)));
    assert!(!apply_git_status(&mut current, snapshot(14)));
    assert_eq!(current.unwrap().branch, "branch-15");
}

#[test]
fn old_conversation_view_defaults_workbench_state() {
    let view: ConversationViewState = serde_json::from_str(r#"{"active_tool":"files"}"#).unwrap();
    assert_eq!(view.workbench, WorkbenchViewState::default());
}

#[derive(Default)]
struct MemoryRepository(Mutex<HashMap<PathBuf, FileData>>);

struct FailingSettingsRepository(Mutex<settings::SettingsDocument>);

impl SettingsRepository for FailingSettingsRepository {
    fn load(&self) -> Result<settings::SettingsDocument, SettingsError> {
        Ok(self.0.lock().unwrap().clone())
    }

    fn save(&self, _: &settings::SettingsDocument) -> Result<(), SettingsError> {
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
    snapshot: Mutex<Option<WorkspaceViewState>>,
    saves: std::sync::atomic::AtomicUsize,
    fail: std::sync::atomic::AtomicBool,
}

impl WorkspaceStateRepository for MemoryState {
    fn load(&self) -> Result<Option<WorkspaceViewState>, WorkspaceError> {
        Ok(self.snapshot.lock().unwrap().clone())
    }

    fn save(&self, snapshot: &WorkspaceViewState) -> Result<(), WorkspaceError> {
        if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(WorkspaceError::new("disk full"));
        }
        self.saves
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        *self.snapshot.lock().unwrap() = Some(snapshot.clone());
        Ok(())
    }
}

#[derive(Default)]
struct MemoryConversationState(Mutex<ConversationState>);

impl ConversationRepository for MemoryConversationState {
    fn load(&self) -> Result<ConversationState, WorkspaceError> {
        Ok(self.0.lock().unwrap().clone())
    }

    fn save(
        &self,
        _expected: &ConversationState,
        projects: &[Project],
        records: &[ConversationRecord],
        active: &ConversationId,
    ) -> Result<(), WorkspaceError> {
        *self.0.lock().unwrap() = (projects.to_vec(), records.to_vec(), Some(active.clone()));
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

impl TerminalBackend for UnavailableTerminalBackend {
    fn start(&self, _cwd: &Path, _columns: u16, _rows: u16) -> Result<TerminalId, TerminalError> {
        Err(TerminalError::new("unavailable"))
    }

    fn input(&self, _id: &TerminalId, _input: &TerminalInput) -> Result<(), TerminalError> {
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
    fn start(&self, _cwd: &Path, _columns: u16, _rows: u16) -> Result<TerminalId, TerminalError> {
        Ok(TerminalId::new("scripted-terminal"))
    }

    fn input(&self, _id: &TerminalId, _input: &TerminalInput) -> Result<(), TerminalError> {
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

struct GapRecoveringTerminalBackend {
    events: Mutex<Vec<TerminalEvent>>,
    attach_count: Mutex<usize>,
}

impl TerminalBackend for GapRecoveringTerminalBackend {
    fn start(&self, _cwd: &Path, _columns: u16, _rows: u16) -> Result<TerminalId, TerminalError> {
        Ok(TerminalId::new("gap-terminal"))
    }

    fn input(&self, _id: &TerminalId, _input: &TerminalInput) -> Result<(), TerminalError> {
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

    fn attach(&self, cwd: &Path, id: &TerminalId) -> Result<TerminalSnapshot, TerminalError> {
        let mut count = self.attach_count.lock().unwrap();
        if *count == 0 {
            *count += 1;
            return Err(TerminalError::new("not attached yet"));
        }
        let mut screen = TerminalScreen::empty(TerminalSize::new(80, 24));
        screen.rows[0] = model_row(80, "resynced");
        Ok(TerminalSnapshot {
            id: id.clone(),
            cwd: cwd.to_owned(),
            status: TerminalStatus::Running,
            revision: 2,
            screen,
        })
    }
}

struct SelectiveAttachBackend;

impl TerminalBackend for SelectiveAttachBackend {
    fn start(&self, _cwd: &Path, _columns: u16, _rows: u16) -> Result<TerminalId, TerminalError> {
        panic!("refresh must never start a terminal")
    }

    fn input(&self, _id: &TerminalId, _input: &TerminalInput) -> Result<(), TerminalError> {
        unreachable!("refresh does not send terminal input")
    }

    fn resize(&self, _id: &TerminalId, _columns: u16, _rows: u16) -> Result<(), TerminalError> {
        unreachable!("refresh does not resize terminals")
    }

    fn poll(&self, id: &TerminalId) -> Result<Vec<TerminalEvent>, TerminalError> {
        if id.as_str() == "recoverable" {
            Ok(vec![TerminalEvent::Delta(model_delta(
                1,
                TerminalSize::new(80, 24),
                "later terminal",
            ))])
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
            revision: 0,
            screen: TerminalScreen::empty(TerminalSize::new(80, 24)),
        })
    }
}

fn model_row(columns: u16, text: &str) -> TerminalRow {
    let mut row = TerminalRow::empty(columns);
    for (column, character) in text.chars().take(columns as usize).enumerate() {
        row.cells[column].text = character.to_string();
    }
    row
}

fn model_delta(revision: u64, size: TerminalSize, text: &str) -> TerminalDelta {
    TerminalDelta {
        revision,
        size,
        changed_rows: vec![TerminalRowUpdate {
            row: 0,
            value: model_row(size.columns, text),
        }],
        cursor: Default::default(),
        modes: Default::default(),
        scrollback: None,
    }
}

impl task::TaskConnection for MemoryTaskBackend {
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

    fn start(&self, workspace: &WorkspaceId, input: TaskStart) -> Result<TaskSnapshot, TaskError> {
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
        settings::SettingsDocument::empty(SettingsScope::User),
    )));
    let settings = SettingsSession::load(repository).unwrap();

    assert!(settings.set_theme("lapis.white".to_owned()).is_err());
    assert_eq!(settings.settings().theme(), "lapis.dark");
    assert!(
        settings
            .set_syntax_theme(Some(settings::SyntaxTheme::Zed))
            .is_err()
    );
    assert_eq!(
        settings.settings().syntax_theme(),
        Some(settings::SyntaxTheme::Lapis)
    );
}

#[test]
fn terminal_appearance_is_saved_in_one_update() {
    #[derive(Default)]
    struct CountingSettingsRepository {
        document: Mutex<Option<settings::SettingsDocument>>,
        saves: std::sync::atomic::AtomicUsize,
    }

    impl SettingsRepository for CountingSettingsRepository {
        fn load(&self) -> Result<settings::SettingsDocument, SettingsError> {
            Ok(self
                .document
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| settings::SettingsDocument::empty(SettingsScope::User)))
        }

        fn save(&self, document: &settings::SettingsDocument) -> Result<(), SettingsError> {
            self.saves.fetch_add(1, Ordering::SeqCst);
            *self.document.lock().unwrap() = Some(document.clone());
            Ok(())
        }
    }

    let repository = Arc::new(CountingSettingsRepository::default());
    let settings = SettingsSession::load(repository.clone()).unwrap();
    settings
        .set_terminal_appearance(
            Some("Menlo".into()),
            Some(15),
            Some(140),
            Some(settings::TerminalCaretShape::Bar),
        )
        .unwrap();
    assert_eq!(repository.saves.load(Ordering::SeqCst), 1);
    let values = settings.settings();
    assert_eq!(values.terminal_font_family().as_deref(), Some("Menlo"));
    assert_eq!(values.terminal_font_size(), Some(15));
    assert_eq!(values.terminal_line_height_percent(), Some(140));
    assert_eq!(
        values.terminal_caret_shape(),
        Some(settings::TerminalCaretShape::Bar)
    );
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

/// Web / Remote の接続経路を模す。表示名は Host の connect 応答だけで決まり、
/// 切替候補の一覧を持たない。
#[derive(Default)]
struct IdentityRepository {
    identity: Mutex<Option<WorkspaceIdentity>>,
    unavailable: std::sync::atomic::AtomicBool,
}

impl IdentityRepository {
    fn with_identity(identity: WorkspaceIdentity) -> Self {
        Self {
            identity: Mutex::new(Some(identity)),
            unavailable: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn fail(&self, fail: bool) {
        self.unavailable
            .store(fail, std::sync::atomic::Ordering::SeqCst);
    }
}

impl WorkspaceRepository for IdentityRepository {
    fn create_directory(&self, _: &Path, _: &str) -> Result<(), WorkspaceError> {
        Err(WorkspaceError::new("Unsupported"))
    }

    fn list_tree(&self, _: &Path) -> Result<Vec<FileEntry>, WorkspaceError> {
        Ok(Vec::new())
    }

    fn identity(&self, _: &Path) -> Result<Option<WorkspaceIdentity>, WorkspaceError> {
        if self.unavailable.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(WorkspaceError::new("workspace.connect failed"));
        }
        Ok(self.identity.lock().unwrap().clone())
    }
}

fn identity_session(repository: Arc<IdentityRepository>) -> EditorSession {
    EditorSession::new(
        repository,
        Arc::new(FixedDialog {
            workspace: None,
            save: None,
        }),
        Arc::new(MemoryState::default()),
        Arc::new(MemoryDocuments::new(Arc::new(MemoryRepository::default()))),
    )
}

fn repo_identity(name: &str, root_name: &str) -> WorkspaceIdentity {
    WorkspaceIdentity {
        id: WorkspaceId::new("workspace-1"),
        name: name.to_owned(),
        root_name: root_name.to_owned(),
    }
}

#[test]
fn connected_workspace_name_comes_from_the_host_identity_without_switch_candidates() {
    let repository = Arc::new(IdentityRepository::with_identity(repo_identity(
        "repo", "checkout",
    )));
    let mut editor = identity_session(repository);
    // Remote の root は profile の label から作る仮想パスで、切替候補は空。
    let root = PathBuf::from("remote/lapis-checkout");

    editor.open_workspace(root).unwrap();

    assert_eq!(editor.workspace_name(), "repo");
    assert_eq!(
        workspace_label(
            Some(editor.workspace_name()),
            editor.workspace_id().map(WorkspaceId::as_str)
        ),
        "Workspace repo (test-workspace)"
    );
}

#[test]
fn identity_failure_does_not_replace_the_workspace_name_with_the_root_name() {
    let repository = Arc::new(IdentityRepository::with_identity(repo_identity(
        "repo", "checkout",
    )));
    repository.fail(true);
    let mut editor = identity_session(repository);

    editor
        .open_workspace(PathBuf::from("remote/lapis-checkout"))
        .unwrap();

    assert_eq!(editor.workspace_name(), "");
    assert_eq!(
        workspace_label(
            Some(editor.workspace_name()),
            editor.workspace_id().map(WorkspaceId::as_str)
        ),
        "Workspace test-workspace"
    );
}

#[test]
fn identity_failure_keeps_the_name_already_known_for_the_workspace() {
    let repository = Arc::new(IdentityRepository::with_identity(repo_identity(
        "repo", "checkout",
    )));
    let mut editor = identity_session(repository.clone());
    let root = PathBuf::from("remote/lapis-checkout");
    editor.open_workspace(root.clone()).unwrap();
    repository.fail(true);

    editor.open_workspace(root).unwrap();

    assert_eq!(editor.workspace_name(), "repo");
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
        .capture(&mut editor, ConversationViewState::default(), None, &[])
        .unwrap();
    let second = conversations
        .create(
            &mut editor,
            ConversationViewState {
                show_panel_controls: false,
                ..ConversationViewState::default()
            },
        )
        .unwrap();
    editor.open_path(second_path).unwrap();
    conversations
        .capture(&mut editor, conversations.active_view(), None, &[])
        .unwrap();

    let first_view = conversations.switch(&first, &mut editor).unwrap();
    assert!(first_view.show_panel_controls);
    assert_eq!(editor.tabs().len(), 1);
    assert_eq!(editor.display_name(), "first.md");

    let second_view = conversations.switch(&second, &mut editor).unwrap();
    assert!(!second_view.show_panel_controls);
    assert_eq!(editor.tabs().len(), 2);
    assert_eq!(editor.display_name(), "second.md");

    let persisted = store.load().unwrap();
    assert_eq!(persisted.1.len(), 2);
    assert_eq!(persisted.2, Some(second));
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
    assert!(
        terminals.terminals()[0]
            .screen
            .plain_text()
            .trim()
            .is_empty()
    );
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
    assert!(
        terminals.terminals()[1].screen.rows[0]
            .plain_text()
            .starts_with("later terminal")
    );
}

#[test]
fn saved_terminal_identity_reattaches_without_starting_a_shell() {
    struct AttachOnly;
    impl TerminalBackend for AttachOnly {
        fn start(&self, _: &Path, _: u16, _: u16) -> Result<TerminalId, TerminalError> {
            panic!("restore must never start a process")
        }
        fn input(&self, _: &TerminalId, _: &TerminalInput) -> Result<(), TerminalError> {
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
        fn attach(&self, cwd: &Path, id: &TerminalId) -> Result<TerminalSnapshot, TerminalError> {
            assert_eq!(id.as_str(), "host-terminal");
            Ok(TerminalSnapshot {
                id: id.clone(),
                cwd: cwd.to_owned(),
                status: TerminalStatus::Running,
                revision: 7,
                screen: {
                    let mut screen = TerminalScreen::empty(TerminalSize::new(100, 28));
                    screen.rows[0] = model_row(100, "already running");
                    screen
                },
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
    assert!(
        session.terminals()[0].screen.rows[0]
            .plain_text()
            .starts_with("already running")
    );
    assert_eq!(session.terminals()[0].revision, 7);
    assert_eq!(session.terminals()[0].status, TerminalStatus::Running);
}

#[test]
fn closing_a_terminal_terminates_the_pty_and_drops_it_from_the_session() {
    struct Counting {
        next: std::sync::atomic::AtomicUsize,
        terminated: Mutex<Vec<TerminalId>>,
    }
    impl TerminalBackend for Counting {
        fn start(&self, _: &Path, _: u16, _: u16) -> Result<TerminalId, TerminalError> {
            Ok(TerminalId::new(format!(
                "terminal-{}",
                self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            )))
        }
        fn input(&self, _: &TerminalId, _: &TerminalInput) -> Result<(), TerminalError> {
            Ok(())
        }
        fn resize(&self, _: &TerminalId, _: u16, _: u16) -> Result<(), TerminalError> {
            Ok(())
        }
        fn poll(&self, _: &TerminalId) -> Result<Vec<TerminalEvent>, TerminalError> {
            Ok(Vec::new())
        }
        fn terminate(&self, id: &TerminalId) -> Result<(), TerminalError> {
            self.terminated.lock().unwrap().push(id.clone());
            Ok(())
        }
    }

    let backend = Arc::new(Counting {
        next: std::sync::atomic::AtomicUsize::new(0),
        terminated: Mutex::new(Vec::new()),
    });
    let mut session = TerminalSession::new(backend.clone());
    let first = session.start(Path::new("workspace"), 80, 24).unwrap();
    let second = session.start(Path::new("workspace"), 80, 24).unwrap();

    session.terminate_and_forget(&first).unwrap();
    assert_eq!(*backend.terminated.lock().unwrap(), vec![first.clone()]);
    assert_eq!(session.terminals().len(), 1);
    assert_eq!(session.terminals()[0].id, second);

    // 既に終了・切断済みの Terminal は PTY を重ねて終了しない。
    session.terminals_mut_for_tests()[0].status = TerminalStatus::Exited;
    session.terminate_and_forget(&second).unwrap();
    assert_eq!(*backend.terminated.lock().unwrap(), vec![first]);
    assert!(session.terminals().is_empty());
}

#[test]
fn terminal_input_batch_preserves_order_and_reports_applied_on_failure() {
    struct Recording {
        inputs: Mutex<Vec<TerminalInput>>,
        fail_at: Mutex<Option<usize>>,
    }
    impl TerminalBackend for Recording {
        fn start(&self, _: &Path, _: u16, _: u16) -> Result<TerminalId, TerminalError> {
            Ok(TerminalId::new("input-terminal"))
        }
        fn input(&self, _: &TerminalId, input: &TerminalInput) -> Result<(), TerminalError> {
            let mut inputs = self.inputs.lock().unwrap();
            if *self.fail_at.lock().unwrap() == Some(inputs.len()) {
                return Err(TerminalError::new("input rejected"));
            }
            inputs.push(input.clone());
            Ok(())
        }
        fn resize(&self, _: &TerminalId, _: u16, _: u16) -> Result<(), TerminalError> {
            Ok(())
        }
        fn poll(&self, _: &TerminalId) -> Result<Vec<TerminalEvent>, TerminalError> {
            Ok(Vec::new())
        }
        fn terminate(&self, _: &TerminalId) -> Result<(), TerminalError> {
            Ok(())
        }
    }

    let backend = Arc::new(Recording {
        inputs: Mutex::new(Vec::new()),
        fail_at: Mutex::new(None),
    });
    let session = TerminalSession::new(backend.clone());
    let id = TerminalId::new("terminal");
    let batch = vec![
        TerminalInput::Text("ab".into()),
        TerminalInput::Key(terminal::TerminalKeyEvent {
            key: terminal::TerminalKey::Enter,
            modifiers: terminal::TerminalModifiers::default(),
        }),
        TerminalInput::Paste("pasted".into()),
    ];

    *backend.fail_at.lock().unwrap() = Some(1);
    let failure = session.input_batch(&id, &batch).unwrap_err();
    assert_eq!(failure.message, "input rejected");
    assert_eq!(failure.applied, 1);
    assert_eq!(*backend.inputs.lock().unwrap(), vec![batch[0].clone()]);

    // 受理済みの Text を再送せず、失敗した Key 以降だけを順序どおり送る。
    *backend.fail_at.lock().unwrap() = None;
    session.input_batch(&id, &batch[failure.applied..]).unwrap();
    assert_eq!(*backend.inputs.lock().unwrap(), batch);
}

#[test]
fn terminal_session_applies_ordered_model_deltas_and_revision() {
    let backend = Arc::new(ScriptedTerminalBackend {
        events: Mutex::new(vec![
            TerminalEvent::Delta(model_delta(1, TerminalSize::new(120, 30), "red")),
            TerminalEvent::Delta(model_delta(2, TerminalSize::new(120, 30), "cursor")),
            TerminalEvent::Delta(model_delta(3, TerminalSize::new(120, 30), "cleared")),
            TerminalEvent::Delta(model_delta(4, TerminalSize::new(120, 30), "e")),
            TerminalEvent::Delta(model_delta(5, TerminalSize::new(120, 30), "é")),
            TerminalEvent::Delta(model_delta(6, TerminalSize::new(120, 30), "done")),
        ]),
    });
    let mut terminals = TerminalSession::new(backend);
    terminals.start(Path::new("repo"), 120, 30).unwrap();

    assert!(terminals.refresh().unwrap());
    let terminal = &terminals.terminals()[0];
    assert_eq!(terminal.screen.rows[0].plain_text().trim_end(), "done");
    assert_eq!(terminal.revision, 6);
}

#[test]
fn terminal_session_resyncs_from_a_snapshot_after_a_delta_gap() {
    let backend = Arc::new(GapRecoveringTerminalBackend {
        events: Mutex::new(vec![TerminalEvent::Delta(model_delta(
            2,
            TerminalSize::new(80, 24),
            "stale delta",
        ))]),
        attach_count: Mutex::new(0),
    });
    let mut terminals = TerminalSession::new(backend);
    terminals.start(Path::new("repo"), 80, 24).unwrap();

    assert!(terminals.refresh().unwrap());
    assert_eq!(terminals.terminals()[0].revision, 2);
    assert_eq!(
        terminals.terminals()[0].screen.rows[0]
            .plain_text()
            .trim_end(),
        "resynced"
    );
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
fn task_session_rejects_non_default_mode_for_the_acp_runner() {
    let workspace = WorkspaceId::new("canonical-workspace");
    let backend = Arc::new(MemoryTaskBackend::default());
    let mut tasks = TaskSession::new(backend.clone());

    let error = tasks
        .start_agent(workspace.clone(), "質問する", TaskMode::Plan)
        .unwrap_err();
    assert!(error.to_string().contains("TaskMode::Default"));
    // 拒否時は実行記録も開始要求も作らない。
    assert!(tasks.records().is_empty());
    assert!(backend.starts.lock().unwrap().is_empty());
}

#[test]
fn task_session_starts_the_acp_runner_with_the_default_mode() {
    let workspace = WorkspaceId::new("canonical-workspace");
    let backend = Arc::new(MemoryTaskBackend::default());
    let mut tasks = TaskSession::new(backend.clone());

    tasks
        .start_agent(workspace, "pong と回答してください", TaskMode::Default)
        .unwrap();
    let starts = backend.starts.lock().unwrap();
    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0].runner.as_deref(), Some(acp_contract::ACP_RUNNER));
    assert_eq!(starts[0].mode, TaskMode::Default);
}
