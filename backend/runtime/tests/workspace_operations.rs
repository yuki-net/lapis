use lapis_backend_state::{
    BackendService, BackendState, EmbeddedClient, WorkspaceEntry, WorkspaceFileBackend,
    WorkspaceRegistration,
};
use lapis_client_api::{
    RequestBody, ResponseBody, SessionId, WorkspaceId, WorkspaceOperation as Operation,
    WorkspaceOperationRequest, WorkspaceOperationResult as Outcome, WorkspaceRelativePath,
};
use lapis_document::{DocumentError, DocumentRepository, FileData, FileFingerprint};
use lapis_editor_core::{ConversationId, ExecutionId, TaskId};
use lapis_git::{FileDiff, GitBackend, GitError, RepositoryStatus, TaskWorktree, WorktreeState};
use lapis_task::{
    Execution, ExecutionStatus, Task, TaskBackend, TaskControl, TaskError, TaskMode, TaskRecord,
    TaskSnapshot, TaskStart,
};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

struct EmptyFiles;
impl DocumentRepository for EmptyFiles {
    fn read_file(&self, _: &Path) -> Result<FileData, DocumentError> {
        Err(DocumentError::io("未使用"))
    }
    fn write_file(
        &self,
        _: &Path,
        _: &[u8],
        _: Option<&FileFingerprint>,
    ) -> Result<FileFingerprint, DocumentError> {
        Err(DocumentError::io("未使用"))
    }
    fn fingerprint(&self, _: &Path) -> Result<Option<FileFingerprint>, DocumentError> {
        Ok(None)
    }
}
impl WorkspaceFileBackend for EmptyFiles {
    fn create_directory(&self, _: &std::path::Path) -> Result<(), lapis_document::DocumentError> {
        Err(lapis_document::DocumentError::io("Unsupported"))
    }
    fn list_children(&self, _: &Path) -> Result<Vec<WorkspaceEntry>, DocumentError> {
        Ok(vec![])
    }
}
#[derive(Default)]
struct FakeTasks {
    records: Mutex<Vec<TaskRecord>>,
    controls: Mutex<Vec<(ExecutionId, TaskControl)>>,
}
impl TaskBackend for FakeTasks {
    fn load(&self) -> Result<Vec<TaskRecord>, TaskError> {
        Ok(self.records.lock().unwrap().clone())
    }
    fn start(&self, record: &TaskRecord) -> Result<(), TaskError> {
        self.records.lock().unwrap().push(record.clone());
        Ok(())
    }
    fn control(&self, id: &ExecutionId, control: &TaskControl) -> Result<(), TaskError> {
        self.controls
            .lock()
            .unwrap()
            .push((id.clone(), control.clone()));
        Ok(())
    }
}
#[derive(Default)]
struct FakeGit {
    trees: Mutex<Vec<TaskWorktree>>,
    watch: Mutex<Option<Arc<std::sync::atomic::AtomicBool>>>,
    notifications: Mutex<std::collections::HashMap<PathBuf, Arc<dyn Fn() + Send + Sync>>>,
    calls: Mutex<Vec<PathBuf>>,
    status_root: Mutex<Option<PathBuf>>,
    fail_discovery: Mutex<bool>,
    unscoped_discovery: Mutex<bool>,
    status_files: Mutex<Vec<lapis_git::ChangedFile>>,
    blocked_status: Mutex<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>>,
    blocked_discard: Mutex<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>>,
}
struct FakeGitWatch(Arc<std::sync::atomic::AtomicBool>);
impl lapis_git::GitWatch for FakeGitWatch {
    fn take_changed(&self) -> bool {
        self.0.swap(false, std::sync::atomic::Ordering::AcqRel)
    }
}
impl GitBackend for FakeGit {
    fn watch_repository(
        &self,
        path: &Path,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Box<dyn lapis_git::GitWatch>, GitError> {
        self.notifications
            .lock()
            .unwrap()
            .insert(path.to_owned(), notify);
        self.watch
            .lock()
            .unwrap()
            .clone()
            .map(|watch| Box::new(FakeGitWatch(watch)) as Box<dyn lapis_git::GitWatch>)
            .ok_or_else(|| GitError::new(lapis_git::GitErrorKind::Io, "watch unavailable"))
    }
    fn execute(
        &self,
        repository: &Path,
        _: lapis_git::GitActionRequest,
    ) -> Result<lapis_git::GitActionResult, GitError> {
        self.calls.lock().unwrap().push(repository.to_owned());
        Ok(lapis_git::GitActionResult {
            diff: None,
            stashes: Vec::new(),
            output: "executed".into(),
        })
    }
    fn status(&self, repository: &Path) -> Result<RepositoryStatus, GitError> {
        let blocked = self.blocked_status.lock().unwrap().take();
        if let Some((entered, release)) = blocked {
            entered.send(()).unwrap();
            release.recv().unwrap();
        }
        self.calls.lock().unwrap().push(repository.to_owned());
        Ok(RepositoryStatus {
            root: self
                .status_root
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| repository.to_owned()),
            branch: "test".into(),
            head: "abc".into(),
            files: self.status_files.lock().unwrap().clone(),
        })
    }
    fn diff(&self, repository: &Path, path: &Path) -> Result<FileDiff, GitError> {
        self.calls.lock().unwrap().push(repository.to_owned());
        Ok(FileDiff {
            path: path.to_owned(),
            additions: 1,
            deletions: 0,
            patch: "+test".into(),
            binary: false,
        })
    }
    fn create_worktree(
        &self,
        repository: &Path,
        task_id: &TaskId,
    ) -> Result<TaskWorktree, GitError> {
        self.calls.lock().unwrap().push(repository.to_owned());
        let path = repository.join("trees").join(task_id.as_str());
        std::fs::create_dir_all(&path).unwrap();
        let tree = TaskWorktree {
            task_id: task_id.clone(),
            repository_root: repository.to_owned(),
            path,
            base_commit: "abc".into(),
            state: WorktreeState::Active,
            conflict: None,
        };
        self.trees.lock().unwrap().push(tree.clone());
        Ok(tree)
    }
    fn worktrees(&self, repository: &Path) -> Result<Vec<TaskWorktree>, GitError> {
        if *self.fail_discovery.lock().unwrap() {
            return Err(GitError::new(
                lapis_git::GitErrorKind::NotRepository,
                "Git なし",
            ));
        }
        Ok(self
            .trees
            .lock()
            .unwrap()
            .iter()
            .filter(|tree| {
                *self.unscoped_discovery.lock().unwrap() || tree.repository_root == repository
            })
            .cloned()
            .collect())
    }
    fn import_file(&self, tree: &TaskWorktree, _: &Path) -> Result<TaskWorktree, GitError> {
        self.calls.lock().unwrap().push(tree.path.clone());
        Ok(tree.clone())
    }
    fn discard_worktree(&self, tree: &TaskWorktree) -> Result<TaskWorktree, GitError> {
        if let Some((entered, release)) = self.blocked_discard.lock().unwrap().take() {
            entered.send(()).unwrap();
            release.recv().unwrap();
        }
        self.calls.lock().unwrap().push(tree.path.clone());
        Ok(tree.clone())
    }
}

#[test]
fn worktree_removal_rejects_active_clients_and_reserves_the_directory_during_git_io() {
    use std::{sync::mpsc, time::Duration};
    let fixture = Fixture::new();
    let parent = fixture.client("parent", "first");
    let Outcome::Worktree(tree) = outcome(execute(&parent, Operation::CreateWorktree)) else {
        panic!("Worktree required")
    };
    let path = fixture
        .service
        .worktree_path(parent.workspace_id().clone(), tree.worktree_id.clone())
        .unwrap();
    let child_id = WorkspaceId::try_new("child").unwrap();
    fixture
        .service
        .register_workspace(WorkspaceRegistration::new(
            child_id.clone(),
            "child",
            path.clone(),
            Arc::new(EmptyFiles),
        ))
        .unwrap();
    let child = fixture.client("child-client", "child");
    fixture.git.calls.lock().unwrap().clear();
    assert!(matches!(
        execute(
            &parent,
            Operation::DiscardWorktree {
                worktree_id: tree.worktree_id.clone()
            }
        ),
        ResponseBody::Error(_)
    ));
    assert!(fixture.git.calls.lock().unwrap().is_empty());
    drop(child);
    let (entered, started) = mpsc::channel();
    let (release, resume) = mpsc::channel();
    *fixture.git.blocked_discard.lock().unwrap() = Some((entered, resume));
    let deleting = std::thread::spawn(move || {
        execute(
            &parent,
            Operation::DiscardWorktree {
                worktree_id: tree.worktree_id,
            },
        )
    });
    started.recv_timeout(Duration::from_secs(2)).unwrap();
    let blocked = EmbeddedClient::connect(
        fixture.service.clone(),
        SessionId::try_new("new-client").unwrap(),
        child_id.clone(),
    );
    assert!(blocked.is_err());
    assert!(
        fixture
            .service
            .register_workspace(WorkspaceRegistration::new(
                WorkspaceId::try_new("new-alias").unwrap(),
                "alias",
                path,
                Arc::new(EmptyFiles)
            ))
            .is_err()
    );
    let unrelated = fixture.client("unrelated", "second");
    assert!(matches!(
        execute(&unrelated, Operation::GitStatus { worktree: None }),
        ResponseBody::WorkspaceExecute(_)
    ));
    release.send(()).unwrap();
    assert!(matches!(
        deleting.join().unwrap(),
        ResponseBody::WorkspaceExecute(_)
    ));
    // FakeGitはdirectoryを残す。予約が完了後に解放されることも確認する。
    assert!(
        EmbeddedClient::connect(
            fixture.service.clone(),
            SessionId::try_new("after-removal").unwrap(),
            child_id
        )
        .is_ok()
    );
}
struct Fixture {
    root: tempfile::TempDir,
    other: tempfile::TempDir,
    service: BackendService,
    git: Arc<FakeGit>,
    tasks: Arc<FakeTasks>,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let git = Arc::new(FakeGit::default());
        let tasks = Arc::new(FakeTasks::default());
        let registrations = [("first", &root), ("second", &other)].map(|(name, directory)| {
            WorkspaceRegistration::new(
                WorkspaceId::try_new(name).unwrap(),
                name,
                directory.path().to_owned(),
                Arc::new(EmptyFiles),
            )
            .with_git(git.clone())
            .with_tasks(tasks.clone())
        });
        let service = BackendService::start(BackendState::new(registrations).unwrap()).unwrap();
        Self {
            root,
            other,
            service,
            git,
            tasks,
        }
    }
    fn client(&self, name: &str, workspace: &str) -> EmbeddedClient {
        EmbeddedClient::connect(
            self.service.clone(),
            SessionId::try_new(name).unwrap(),
            WorkspaceId::try_new(workspace).unwrap(),
        )
        .unwrap()
    }
    fn record(&self, name: &str) -> TaskRecord {
        TaskRecord::new(
            Task {
                id: TaskId::new(name),
                conversation_id: ConversationId::new("conversation"),
                title: "既存タスク".into(),
                prompt: "テスト".into(),
                created_at_ms: 1,
            },
            Execution {
                id: ExecutionId::new(format!("execution-{name}")),
                task_id: TaskId::new(name),
                workspace_id: lapis_editor_core::WorkspaceId::new("obsolete-id"),
                workspace_root: self.root.path().canonicalize().unwrap(),
                runner: "codex-app-server".into(),
                mode: TaskMode::Default,
                status: ExecutionStatus::Queued,
                started_at_ms: None,
                completed_at_ms: None,
                external_thread_id: None,
                failure: None,
            },
        )
    }
}
fn input(isolated: bool) -> TaskStart {
    TaskStart {
        prompt: "回帰テスト\n指示".into(),
        mode: TaskMode::Plan,
        isolated,
        conversation_id: ConversationId::new("conversation"),
    }
}
fn body(workspace: &WorkspaceId, operation: Operation) -> RequestBody {
    RequestBody::WorkspaceExecute(WorkspaceOperationRequest {
        workspace_id: workspace.clone(),
        operation,
    })
}
fn execute(client: &EmbeddedClient, operation: Operation) -> ResponseBody {
    client
        .request(body(client.workspace_id(), operation))
        .unwrap()
}
fn outcome(response: ResponseBody) -> Outcome {
    match response {
        ResponseBody::WorkspaceExecute(value) => value,
        other => panic!("操作失敗: {other:?}"),
    }
}
fn denied(response: ResponseBody) {
    let ResponseBody::Error(error) = response else {
        panic!("拒否されなかった: {response:?}")
    };
    assert_eq!(error.code.as_str(), lapis_client_api::FORBIDDEN);
}

#[test]
fn git_actions_accept_deleted_parent_directories_without_relaxing_containment() {
    let fixture = Fixture::new();
    let client = fixture.client("deleted-path", "first");
    let stage = |path: &str| Operation::GitAction {
        worktree: None,
        request: lapis_git::GitActionRequest {
            action: lapis_git::GitAction::Stage { path: path.into() },
            expected_head: Some("abc".into()),
        },
    };
    assert!(matches!(
        execute(&client, stage("deleted/directory/note.txt")),
        ResponseBody::WorkspaceExecute(_)
    ));
    assert!(matches!(
        execute(&client, stage("../outside.txt")),
        ResponseBody::Error(_)
    ));
    std::fs::write(fixture.root.path().join("file"), "content").unwrap();
    assert!(matches!(
        execute(&client, stage("file/note.txt")),
        ResponseBody::Error(_)
    ));
}
fn started(client: &EmbeddedClient, isolated: bool) -> TaskSnapshot {
    let Outcome::TaskStarted(value) = outcome(execute(
        client,
        Operation::StartTask {
            input: input(isolated),
        },
    )) else {
        panic!("開始結果が必要")
    };
    *value
}
fn tasks(client: &EmbeddedClient) -> Vec<TaskSnapshot> {
    let Outcome::Tasks(value) = outcome(execute(client, Operation::Tasks)) else {
        panic!("一覧が必要")
    };
    value
}

#[test]
fn request_capabilities_separate_git_reads_writes_and_task_controls() {
    let workspace = WorkspaceId::try_new("first").unwrap();
    let id = TaskId::new("tree");
    let path = WorkspaceRelativePath::parse("note.txt").unwrap();
    for (operation, expected) in [
        (
            Operation::GitAction {
                worktree: None,
                request: lapis_git::GitActionRequest {
                    action: lapis_git::GitAction::History,
                    expected_head: None,
                },
            },
            "workspace.git.read",
        ),
        (
            Operation::GitAction {
                worktree: None,
                request: lapis_git::GitActionRequest {
                    action: lapis_git::GitAction::Stage {
                        path: "note.txt".into(),
                    },
                    expected_head: Some("abc".into()),
                },
            },
            "workspace.git.write",
        ),
        (
            Operation::GitStatus { worktree: None },
            "workspace.git.read",
        ),
        (
            Operation::GitDiff {
                path: path.clone(),
                worktree: Some(id.clone()),
            },
            "workspace.git.read",
        ),
        (Operation::Worktrees, "workspace.git.read"),
        (Operation::CreateWorktree, "workspace.git.write"),
        (
            Operation::ImportFile {
                worktree_id: id.clone(),
                path,
            },
            "workspace.git.write",
        ),
        (
            Operation::DiscardWorktree { worktree_id: id },
            "workspace.git.write",
        ),
        (Operation::Tasks, "workspace.tasks.read"),
        (
            Operation::StartTask {
                input: input(false),
            },
            "workspace.tasks.start",
        ),
        (
            Operation::ControlTask {
                execution_id: ExecutionId::new("execution"),
                control: TaskControl::Cancel,
            },
            "workspace.tasks.control",
        ),
    ] {
        assert_eq!(
            operation.is_mutation(),
            matches!(
                expected,
                "workspace.git.write" | "workspace.tasks.start" | "workspace.tasks.control"
            )
        );
        let request = body(&workspace, operation);
        assert_eq!(request.required_capability(), expected);
        assert_eq!(request.workspace_id(), Some(&workspace));
    }
}

#[test]
fn clients_share_tasks_worktrees_and_survive_all_connections_dropping() {
    let fixture = Fixture::new();
    let first = fixture.client("one", "first");
    let second = fixture.client("two", "first");
    let task = started(&first, true);
    assert_eq!(tasks(&first), vec![task.clone()]);
    assert_eq!(tasks(&second), vec![task.clone()]);
    let trees = outcome(execute(&first, Operation::Worktrees));
    assert_eq!(outcome(execute(&second, Operation::Worktrees)), trees);
    drop(first);
    drop(second);
    fixture.tasks.records.lock().unwrap()[0].set_status(ExecutionStatus::Running);
    let reconnected = fixture.client("reconnected", "first");
    let running = tasks(&reconnected);
    assert_eq!(running[0].execution.id, task.execution.id);
    assert_eq!(running[0].execution.status, ExecutionStatus::Running);
    assert_eq!(outcome(execute(&reconnected, Operation::Worktrees)), trees);
    assert!(fixture.tasks.controls.lock().unwrap().is_empty());
    fixture.tasks.records.lock().unwrap()[0].set_status(ExecutionStatus::Succeeded);
    assert_eq!(
        tasks(&reconnected)[0].execution.status,
        ExecutionStatus::Succeeded
    );
    denied(execute(
        &reconnected,
        Operation::ControlTask {
            execution_id: task.execution.id,
            control: TaskControl::Cancel,
        },
    ));
    assert!(fixture.tasks.controls.lock().unwrap().is_empty());
}

#[test]
fn identical_start_inputs_receive_unique_server_ids_across_clients_and_workspaces() {
    let fixture = Fixture::new();
    let first = fixture.client("one", "first");
    let second = fixture.client("two", "first");
    let other = fixture.client("other", "second");
    let mut seeded = fixture.record("persisted-task");
    seeded.execution.id = ExecutionId::new("persisted-execution");
    fixture.tasks.records.lock().unwrap().push(seeded);
    let results = [
        started(&first, false),
        started(&second, false),
        started(&other, false),
    ];
    for (index, result) in results.iter().enumerate() {
        assert_ne!(result.task.id.as_str(), "persisted-task");
        assert_ne!(result.execution.id.as_str(), "persisted-execution");
        assert_eq!(result.execution.task_id, result.task.id);
        for previous in &results[..index] {
            assert_ne!(previous.task.id, result.task.id);
            assert_ne!(previous.execution.id, result.execution.id);
        }
    }
    assert_eq!(fixture.tasks.load().unwrap().len(), 4);
    assert_eq!(results[0].execution.workspace_id.as_str(), "first");
    assert_eq!(results[2].execution.workspace_id.as_str(), "second");
}

#[test]
fn foreign_workspace_task_and_worktree_operations_are_denied_before_backend_calls() {
    let fixture = Fixture::new();
    let first = fixture.client("one", "first");
    let second = fixture.client("two", "second");
    let task = started(&first, true);
    let tree = task.execution.worktree_id.unwrap();
    fixture.git.calls.lock().unwrap().clear();
    for operation in [
        Operation::GitAction {
            worktree: None,
            request: lapis_git::GitActionRequest {
                action: lapis_git::GitAction::History,
                expected_head: None,
            },
        },
        Operation::Tasks,
        Operation::Worktrees,
        Operation::GitStatus { worktree: None },
        Operation::StartTask {
            input: input(false),
        },
    ] {
        denied(
            second
                .request(body(first.workspace_id(), operation))
                .unwrap(),
        );
    }
    assert!(tasks(&second).is_empty());
    for operation in [
        Operation::GitAction {
            worktree: Some(tree.clone()),
            request: lapis_git::GitActionRequest {
                action: lapis_git::GitAction::Stage {
                    path: "note.txt".into(),
                },
                expected_head: Some("abc".into()),
            },
        },
        Operation::GitStatus {
            worktree: Some(tree.clone()),
        },
        Operation::GitDiff {
            worktree: Some(tree.clone()),
            path: WorkspaceRelativePath::parse("note.txt").unwrap(),
        },
        Operation::ImportFile {
            worktree_id: tree.clone(),
            path: WorkspaceRelativePath::parse("note.txt").unwrap(),
        },
        Operation::DiscardWorktree { worktree_id: tree },
        Operation::ControlTask {
            execution_id: task.execution.id,
            control: TaskControl::Cancel,
        },
    ] {
        denied(execute(&second, operation));
    }
    assert!(fixture.git.calls.lock().unwrap().is_empty());
    assert!(fixture.tasks.controls.lock().unwrap().is_empty());
    assert_eq!(fixture.tasks.load().unwrap().len(), 1);
}

#[test]
fn task_start_rejects_empty_prompts_and_unsafe_conversation_ids_before_side_effects() {
    let fixture = Fixture::new();
    let client = fixture.client("one", "first");
    let mut empty = input(true);
    empty.prompt = " \n\t".into();
    denied(execute(&client, Operation::StartTask { input: empty }));
    for invalid in ["", "..", "../escape", "..\\escape", "日本語"] {
        let mut start = input(true);
        start.conversation_id = ConversationId::new(invalid);
        denied(execute(&client, Operation::StartTask { input: start }));
    }
    assert!(fixture.git.calls.lock().unwrap().is_empty());
    assert!(fixture.tasks.load().unwrap().is_empty());
}

#[test]
fn task_and_execution_id_traversal_is_rejected_before_backend_calls() {
    let fixture = Fixture::new();
    let client = fixture.client("one", "first");
    for invalid in [
        "",
        ".",
        "..",
        "../escape",
        "..\\escape",
        "/absolute",
        "C:\\escape",
        "nested/task",
        "nested\\task",
    ] {
        denied(execute(
            &client,
            Operation::GitStatus {
                worktree: Some(TaskId::new(invalid)),
            },
        ));
        denied(execute(
            &client,
            Operation::DiscardWorktree {
                worktree_id: TaskId::new(invalid),
            },
        ));
        denied(execute(
            &client,
            Operation::ControlTask {
                execution_id: ExecutionId::new(invalid),
                control: TaskControl::Cancel,
            },
        ));
    }
    assert!(fixture.git.calls.lock().unwrap().is_empty());
    assert!(fixture.tasks.controls.lock().unwrap().is_empty());
}

#[test]
fn parent_repository_cannot_be_exposed_by_a_nested_workspace() {
    let fixture = Fixture::new();
    let client = fixture.client("one", "first");
    *fixture.git.status_root.lock().unwrap() =
        Some(fixture.root.path().parent().unwrap().to_owned());
    for operation in [
        Operation::GitStatus { worktree: None },
        Operation::GitDiff {
            path: WorkspaceRelativePath::parse("note.txt").unwrap(),
            worktree: None,
        },
        Operation::Worktrees,
        Operation::CreateWorktree,
        Operation::StartTask { input: input(true) },
    ] {
        denied(execute(&client, operation));
    }
    assert!(fixture.git.trees.lock().unwrap().is_empty());
    assert!(fixture.tasks.load().unwrap().is_empty());
}

#[test]
fn task_start_constructs_authoritative_metadata_without_client_execution_state() {
    let fixture = Fixture::new();
    let client = fixture.client("one", "first");
    let snapshot = started(&client, false);
    let records = fixture.tasks.load().unwrap();
    let record = &records[0];
    assert_eq!(
        record.execution.workspace_root,
        fixture.root.path().canonicalize().unwrap()
    );
    assert_eq!(record.execution.workspace_id.as_str(), "first");
    assert_eq!(record.execution.status, ExecutionStatus::Queued);
    assert_eq!(record.execution.runner, "codex-app-server");
    assert_eq!(record.execution.mode, TaskMode::Plan);
    assert_eq!(record.execution.started_at_ms, None);
    assert_eq!(record.execution.completed_at_ms, None);
    assert_eq!(record.execution.failure, None);
    assert_eq!(record.execution.external_thread_id, None);
    assert!(record.events.is_empty());
    assert_eq!(record.task.id, snapshot.task.id);
    assert_eq!(record.task.conversation_id, input(false).conversation_id);
    assert_eq!(record.task.prompt, input(false).prompt);
    assert_eq!(snapshot.execution.worktree_id, None);
}

#[test]
fn conversation_id_length_boundaries_and_server_worktree_ids() {
    let fixture = Fixture::new();
    let client = fixture.client("one", "first");
    let mut invalid = input(false);
    invalid.conversation_id = ConversationId::new("a".repeat(129));
    denied(execute(&client, Operation::StartTask { input: invalid }));
    let mut valid = input(false);
    valid.conversation_id = ConversationId::new(format!("Aa0-_{}", "z".repeat(123)));
    outcome(execute(&client, Operation::StartTask { input: valid }));
    let Outcome::Worktree(first) = outcome(execute(&client, Operation::CreateWorktree)) else {
        panic!("worktree が必要")
    };
    let Outcome::Worktree(second) = outcome(execute(&client, Operation::CreateWorktree)) else {
        panic!("worktree が必要")
    };
    assert_ne!(first.worktree_id, second.worktree_id);
    assert!(
        first
            .worktree_id
            .as_str()
            .bytes()
            .all(|value| value.is_ascii_alphanumeric() || value == b'-' || value == b'_')
    );
}

#[test]
fn task_listing_survives_missing_git_and_failed_worktree_discovery() {
    let fixture = Fixture::new();
    let client = fixture.client("one", "first");
    let task = started(&client, false);
    *fixture.git.fail_discovery.lock().unwrap() = true;
    assert_eq!(tasks(&client), vec![task.clone()]);
    let state = BackendState::new([WorkspaceRegistration::new(
        WorkspaceId::try_new("first").unwrap(),
        "no Git",
        fixture.root.path().to_owned(),
        Arc::new(EmptyFiles),
    )
    .with_tasks(fixture.tasks.clone())])
    .unwrap();
    let service = BackendService::start(state).unwrap();
    let without_git = EmbeddedClient::connect(
        service,
        SessionId::try_new("no-git").unwrap(),
        WorkspaceId::try_new("first").unwrap(),
    )
    .unwrap();
    assert_eq!(tasks(&without_git), vec![task]);
}

#[test]
fn task_controls_accept_only_nonterminal_statuses_in_the_workspace() {
    let fixture = Fixture::new();
    let client = fixture.client("one", "first");
    for (index, status) in [
        ExecutionStatus::Queued,
        ExecutionStatus::Running,
        ExecutionStatus::WaitingForInput,
        ExecutionStatus::WaitingForApproval,
        ExecutionStatus::Succeeded,
        ExecutionStatus::Failed,
        ExecutionStatus::Cancelled,
    ]
    .into_iter()
    .enumerate()
    {
        let mut record = fixture.record(&format!("status-{index}"));
        record.execution.status = status;
        fixture.tasks.records.lock().unwrap().push(record.clone());
        let response = execute(
            &client,
            Operation::ControlTask {
                execution_id: record.execution.id,
                control: TaskControl::Cancel,
            },
        );
        if status.is_terminal() {
            denied(response);
        } else {
            assert_eq!(outcome(response), Outcome::Completed);
        }
    }
    assert_eq!(fixture.tasks.controls.lock().unwrap().len(), 4);
}

#[test]
fn isolated_start_uses_runtime_worktree_and_unregistered_subdirectory_is_not_a_task_scope() {
    let fixture = Fixture::new();
    let client = fixture.client("one", "first");
    let task = started(&client, true);
    let tree = fixture.git.trees.lock().unwrap()[0].clone();
    let record = fixture.tasks.load().unwrap()[0].clone();
    assert_eq!(record.execution.workspace_root, tree.path);
    assert_eq!(task.execution.worktree_id, Some(tree.task_id));
    assert_eq!(tasks(&client), vec![task]);
    let mut nested = fixture.record("unregistered");
    nested.execution.workspace_root = fixture.root.path().join("nested");
    std::fs::create_dir(&nested.execution.workspace_root).unwrap();
    fixture.tasks.records.lock().unwrap().push(nested);
    assert_eq!(tasks(&client).len(), 1);
}

#[test]
fn parent_repository_worktrees_cannot_authorize_nested_workspace_task_access() {
    let fixture = Fixture::new();
    let client = fixture.client("one", "first");
    let mut record = fixture.record("parent-task");
    record.execution.workspace_root = fixture.other.path().canonicalize().unwrap();
    fixture.tasks.records.lock().unwrap().push(record.clone());
    fixture.git.trees.lock().unwrap().push(TaskWorktree {
        task_id: record.task.id.clone(),
        repository_root: fixture.root.path().parent().unwrap().to_owned(),
        path: record.execution.workspace_root,
        base_commit: "abc".into(),
        state: WorktreeState::Active,
        conflict: None,
    });
    *fixture.git.unscoped_discovery.lock().unwrap() = true;
    assert!(tasks(&client).is_empty());
    denied(execute(
        &client,
        Operation::ControlTask {
            execution_id: record.execution.id,
            control: TaskControl::Cancel,
        },
    ));
    denied(execute(
        &client,
        Operation::DiscardWorktree {
            worktree_id: record.task.id,
        },
    ));
    assert!(fixture.tasks.controls.lock().unwrap().is_empty());
    assert!(fixture.git.calls.lock().unwrap().is_empty());
}

#[test]
fn git_snapshots_reject_absolute_backend_file_paths() {
    let fixture = Fixture::new();
    let client = fixture.client("one", "first");
    fixture
        .git
        .status_files
        .lock()
        .unwrap()
        .push(lapis_git::ChangedFile {
            path: fixture.other.path().join("secret.txt"),
            old_path: None,
            kind: lapis_git::ChangeKind::Modified,
            staged: false,
        });
    denied(execute(&client, Operation::GitStatus { worktree: None }));
}

#[test]
fn slow_git_does_not_block_documents_other_workspaces_or_terminal_polling() {
    use std::sync::mpsc;
    use std::time::Duration;
    struct Terminal(mpsc::Sender<()>);
    impl lapis_terminal::TerminalBackend for Terminal {
        fn start(
            &self,
            _: &Path,
            _: u16,
            _: u16,
        ) -> Result<lapis_terminal::TerminalId, lapis_terminal::TerminalError> {
            Ok(lapis_terminal::TerminalId::new("native"))
        }
        fn input(
            &self,
            _: &lapis_terminal::TerminalId,
            _: &lapis_terminal::TerminalInput,
        ) -> Result<(), lapis_terminal::TerminalError> {
            Ok(())
        }
        fn resize(
            &self,
            _: &lapis_terminal::TerminalId,
            _: u16,
            _: u16,
        ) -> Result<lapis_terminal::TerminalDelta, lapis_terminal::TerminalError> {
            Ok(lapis_terminal::TerminalDelta {
                revision: 1,
                size: lapis_terminal::TerminalSize::new(80, 24),
                changed_rows: Vec::new(),
                cursor: Default::default(),
                modes: Default::default(),
                scrollback: None,
            })
        }
        fn terminate(
            &self,
            _: &lapis_terminal::TerminalId,
        ) -> Result<(), lapis_terminal::TerminalError> {
            Ok(())
        }
        fn poll(
            &self,
            _: &lapis_terminal::TerminalId,
        ) -> Result<Vec<lapis_terminal::TerminalEvent>, lapis_terminal::TerminalError> {
            let _ = self.0.send(());
            Ok(Vec::new())
        }
    }
    let root = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let git = Arc::new(FakeGit::default());
    let (polled, polls) = mpsc::channel();
    let terminal = Arc::new(Terminal(polled));
    let registrations = [("first", &root), ("second", &other)].map(|(name, directory)| {
        WorkspaceRegistration::new(
            WorkspaceId::try_new(name).unwrap(),
            name,
            directory.path().to_owned(),
            Arc::new(EmptyFiles),
        )
        .with_git(git.clone())
        .with_terminal(terminal.clone())
    });
    let service = BackendService::start(BackendState::new(registrations).unwrap()).unwrap();
    let connect = |name: &str, workspace: &str| {
        EmbeddedClient::connect(
            service.clone(),
            SessionId::try_new(name).unwrap(),
            WorkspaceId::try_new(workspace).unwrap(),
        )
        .unwrap()
    };
    let slow = connect("slow-git", "first");
    let documents = connect("documents", "first");
    let second = connect("other-workspace", "second");
    assert!(matches!(
        documents
            .request(RequestBody::TerminalStart(
                lapis_client_api::TerminalStartRequest {
                    workspace_id: documents.workspace_id().clone(),
                    cwd: None,
                    command: None,
                    shell: Default::default(),
                    size: lapis_client_api::TerminalSize {
                        columns: 80,
                        rows: 24
                    },
                }
            ))
            .unwrap(),
        ResponseBody::TerminalStart(_)
    ));
    let (entered, started) = mpsc::channel();
    let (release, resume) = mpsc::channel();
    *git.blocked_status.lock().unwrap() = Some((entered, resume));
    let slow_thread =
        std::thread::spawn(move || execute(&slow, Operation::GitStatus { worktree: None }));
    started.recv_timeout(Duration::from_secs(2)).unwrap();
    while polls.try_recv().is_ok() {}
    let (respond, receive) = mpsc::channel();
    let document_thread = std::thread::spawn(move || {
        let document = documents
            .request(RequestBody::DocumentUntitled(
                lapis_client_api::DocumentUntitledRequest {
                    workspace_id: documents.workspace_id().clone(),
                },
            ))
            .unwrap();
        let other = execute(&second, Operation::GitStatus { worktree: None });
        respond.send((document, other)).unwrap();
    });
    let independent = receive.recv_timeout(Duration::from_secs(2));
    let polling = polls.recv_timeout(Duration::from_secs(2));
    release.send(()).unwrap();
    assert!(matches!(
        slow_thread.join().unwrap(),
        ResponseBody::WorkspaceExecute(_)
    ));
    document_thread.join().unwrap();
    let (document, other) = independent.expect("Git I/O must not hold the Runtime worker");
    assert!(matches!(document, ResponseBody::DocumentUntitled(_)));
    assert!(matches!(other, ResponseBody::WorkspaceExecute(_)));
    polling.expect("Terminal polling must continue while Git is blocked");
}

#[test]
fn operation_errors_are_distinguishable_without_exposing_native_details() {
    let fixture = Fixture::new();
    *fixture.git.status_root.lock().unwrap() = Some(fixture.other.path().canonicalize().unwrap());
    let client = fixture.client("errors", "first");
    assert!(matches!(
        execute(&client, Operation::GitStatus { worktree: None }),
        ResponseBody::Error(_)
    ));
    *fixture.git.fail_discovery.lock().unwrap() = true;
    let response = execute(
        &client,
        Operation::GitStatus {
            worktree: Some(TaskId::new("missing")),
        },
    );
    let ResponseBody::Error(error) = response else {
        panic!("Expected error")
    };
    assert_eq!(error.detail.as_deref(), Some("Git repository not found"));
    assert!(
        !error
            .detail
            .unwrap()
            .contains(&fixture.root.path().to_string_lossy().to_string())
    );
}

#[test]
fn git_status_cache_refreshes_on_watch_changes_and_operations() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let fixture = Fixture::new();
    let dirty = Arc::new(AtomicBool::new(false));
    *fixture.git.watch.lock().unwrap() = Some(dirty.clone());
    let client = fixture.client("cache-client", "first");
    let status = || outcome(execute(&client, Operation::GitStatus { worktree: None }));
    let Outcome::GitStatus(initial) = status() else {
        panic!("status required")
    };
    let Outcome::GitStatus(cached) = status() else {
        panic!("status required")
    };
    assert_eq!(initial.revision, cached.revision);
    assert_eq!(fixture.git.calls.lock().unwrap().len(), 1);
    *fixture.git.status_files.lock().unwrap() = vec![lapis_git::ChangedFile {
        path: PathBuf::from("draft.txt"),
        old_path: None,
        kind: lapis_git::ChangeKind::Modified,
        staged: false,
    }];
    dirty.store(true, Ordering::Release);
    let Outcome::GitStatus(updated) = status() else {
        panic!("status required")
    };
    assert_eq!(updated.files[0].path, "draft.txt");
    assert!(updated.revision > initial.revision);
    assert_eq!(fixture.git.calls.lock().unwrap().len(), 2);
    let request = lapis_git::GitActionRequest {
        action: lapis_git::GitAction::Stage {
            path: "draft.txt".into(),
        },
        expected_head: Some("abc".into()),
    };
    assert!(matches!(
        outcome(execute(
            &client,
            Operation::GitAction {
                worktree: None,
                request
            }
        )),
        Outcome::GitAction(_)
    ));
    let calls_after_operation = fixture.git.calls.lock().unwrap().len();
    assert_eq!(calls_after_operation, 5);
    assert!(matches!(status(), Outcome::GitStatus(_)));
    assert_eq!(
        fixture.git.calls.lock().unwrap().len(),
        calls_after_operation
    );
}

#[test]
fn git_status_without_watcher_uses_bounded_fallback_cache() {
    let fixture = Fixture::new();
    let client = fixture.client("fallback-cache", "first");
    for _ in 0..2 {
        assert!(matches!(
            outcome(execute(&client, Operation::GitStatus { worktree: None })),
            Outcome::GitStatus(_)
        ));
    }
    assert_eq!(fixture.git.calls.lock().unwrap().len(), 1);
}

#[test]
fn git_events_debounce_and_refresh_only_the_changed_repository() {
    use std::{
        sync::atomic::{AtomicBool, Ordering},
        time::Duration,
    };
    let fixture = Fixture::new();
    let dirty = Arc::new(AtomicBool::new(false));
    *fixture.git.watch.lock().unwrap() = Some(dirty.clone());
    for name in ["frontend", "backend"] {
        std::fs::create_dir(fixture.root.path().join(name)).unwrap();
    }
    let client = fixture.client("git-events", "first");
    let events = client.subscribe().unwrap();
    for name in ["frontend", "backend"] {
        assert!(matches!(
            outcome(execute(
                &client,
                Operation::RepositoryGitStatus {
                    repository_id: format!("path:{name}")
                }
            )),
            Outcome::RepositoryGitStatus { .. }
        ));
    }
    let mut initialized = std::collections::HashSet::new();
    while initialized.len() < 2 {
        let event = events.recv_timeout(Duration::from_secs(3)).unwrap();
        if let lapis_client_api::EventBody::GitStatusChanged { repository, .. } = event.body {
            initialized.insert(repository.repository_id);
        }
    }
    fixture.git.calls.lock().unwrap().clear();
    *fixture.git.status_files.lock().unwrap() = vec![lapis_git::ChangedFile {
        path: PathBuf::from("changed"),
        old_path: None,
        kind: lapis_git::ChangeKind::Modified,
        staged: false,
    }];
    dirty.store(true, Ordering::Release);
    let frontend = fixture.root.path().join("frontend").canonicalize().unwrap();
    let callback = fixture
        .git
        .notifications
        .lock()
        .unwrap()
        .get(&frontend)
        .unwrap()
        .clone();
    for _ in 0..100 {
        callback();
    }
    let event = events.recv_timeout(Duration::from_secs(3)).unwrap();
    let lapis_client_api::EventBody::GitStatusChanged { repository, .. } = event.body else {
        panic!("Git event required");
    };
    assert_eq!(repository.repository_id, "path:frontend");
    assert_eq!(repository.status.files[0].path, "changed");
    let calls = fixture.git.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0], frontend);
}

#[test]
fn git_watch_unavailable_refreshes_in_host_fallback() {
    use std::time::Duration;
    let fixture = Fixture::new();
    let client = fixture.client("git-fallback", "first");
    let events = client.subscribe().unwrap();
    assert!(matches!(
        outcome(execute(&client, Operation::GitStatus { worktree: None })),
        Outcome::GitStatus(_)
    ));
    let initial = events.recv_timeout(Duration::from_secs(3)).unwrap();
    let lapis_client_api::EventBody::GitStatusChanged { repository, .. } = initial.body else {
        panic!("initial Git event required")
    };
    *fixture.git.status_files.lock().unwrap() = vec![lapis_git::ChangedFile {
        path: PathBuf::from("later"),
        old_path: None,
        kind: lapis_git::ChangeKind::Modified,
        staged: false,
    }];
    let changed = events.recv_timeout(Duration::from_secs(18)).unwrap();
    let lapis_client_api::EventBody::GitStatusChanged {
        repository: updated,
        ..
    } = changed.body
    else {
        panic!("fallback Git event required")
    };
    assert!(updated.status.revision > repository.status.revision);
    assert_eq!(updated.status.files[0].path, "later");
}
