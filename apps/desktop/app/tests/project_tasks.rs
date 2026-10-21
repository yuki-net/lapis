use lapis_desktop_client::{
    ConversationSession, EditorSession, SourceRole, TerminalSession, WorkspaceDialog,
};
use lapis_host::BackendHost;
use lapis_persistence::{LocalConversationRepository, LocalWorkspaceStateRepository};
use std::{path::PathBuf, sync::Arc};

struct NoDialog;
impl WorkspaceDialog for NoDialog {
    fn choose_workspace_path(&self) -> Option<PathBuf> {
        None
    }
    fn choose_save_path(&self, _: &str) -> Option<PathBuf> {
        None
    }
}

fn git(root: &std::path::Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn task_worktree_roundtrip_preserves_drafts_terminals_and_source_identity() {
    let directory = tempfile::tempdir().unwrap();
    let primary = directory.path().join("primary");
    let sub = directory.path().join("sub");
    let tree_path = directory.path().join("topic");
    for root in [&primary, &sub] {
        std::fs::create_dir(root).unwrap();
        git(root, &["init", "-b", "main"]);
        git(root, &["config", "user.name", "Lapis Test"]);
        git(root, &["config", "user.email", "test@example.invalid"]);
        git(root, &["config", "core.autocrlf", "false"]);
        git(
            root,
            &[
                "config",
                "core.hooksPath",
                directory.path().join("no-hooks").to_str().unwrap(),
            ],
        );
        std::fs::write(root.join("note.ts"), "base").unwrap();
        git(root, &["add", "note.ts"]);
        git(root, &["commit", "-m", "initial"]);
    }
    git(
        &primary,
        &[
            "worktree",
            "add",
            "-b",
            "topic",
            tree_path.to_str().unwrap(),
        ],
    );
    let primary = primary.canonicalize().unwrap();
    let sub = sub.canonicalize().unwrap();
    let tree_path = tree_path.canonicalize().unwrap();
    let host = BackendHost::start().unwrap();
    let mut editor = EditorSession::new_empty(
        host.workspaces(),
        Arc::new(NoDialog),
        Arc::new(LocalWorkspaceStateRepository::new(
            directory.path().join("editor.json"),
        )),
        host.documents(),
    );
    editor.open_workspace(primary.clone()).unwrap();
    let repository = Arc::new(LocalConversationRepository::new(
        directory.path().join("tasks.json"),
    ));
    let mut tasks = ConversationSession::new(repository.clone(), editor.snapshot());
    let project = tasks.projects()[0].id().clone();
    tasks
        .register_source(&editor, &project, sub.clone())
        .unwrap();
    let task = tasks
        .create_task(&mut editor, &project, primary.clone(), Some(sub.clone()))
        .unwrap();
    let connection = host.git();
    let trees = connection
        .worktrees(editor.workspace_id().unwrap())
        .unwrap();
    assert_eq!(trees.len(), 1);
    let tree = trees[0].worktree_id.clone();
    editor.open_path(primary.join("note.ts")).unwrap();
    editor.replace_range(0..4, "primary draft").unwrap();
    let primary_document = editor.active_document_id().cloned();
    let mut terminal = TerminalSession::new(host.terminals());
    let id = terminal.start(&primary, 80, 24).unwrap();
    tasks
        .capture(&editor, Default::default(), None, terminal.terminals())
        .unwrap();
    tasks
        .set_worktree(&mut editor, connection.as_ref(), Some(&tree))
        .unwrap();
    assert_eq!(editor.workspace_root(), Some(tree_path.as_path()));
    assert_eq!(tasks.active_id(), &task);
    assert_eq!(tasks.projects()[0].sources().count(), 2);
    assert!(tasks.active_record().unwrap().terminals.is_empty());
    editor.open_path(tree_path.join("note.ts")).unwrap();
    editor.replace_range(0..4, "worktree draft").unwrap();
    let tree_document = editor.active_document_id().cloned();
    tasks
        .capture(&editor, Default::default(), None, &[])
        .unwrap();
    tasks.switch_source(&mut editor, SourceRole::Sub).unwrap();
    assert!(
        tasks
            .set_worktree(&mut editor, connection.as_ref(), Some(&tree))
            .is_err()
    );
    assert_eq!(editor.workspace_root(), Some(sub.as_path()));
    tasks
        .switch_source(&mut editor, SourceRole::Primary)
        .unwrap();
    assert_eq!(editor.active_text().unwrap(), "worktree draft");
    tasks
        .set_worktree(&mut editor, connection.as_ref(), None)
        .unwrap();
    assert_eq!(editor.active_document_id(), primary_document.as_ref());
    assert_eq!(editor.active_text().unwrap(), "primary draft");
    assert_eq!(tasks.active_record().unwrap().terminals.len(), 1);
    terminal
        .input(
            &id,
            if cfg!(windows) {
                "Write-Output ('WORKTREE_' + 'ALIVE')\r"
            } else {
                "printf 'WORKTREE_%s\\n' ALIVE\r"
            },
        )
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        terminal.refresh().unwrap();
        if terminal.terminals()[0]
            .output
            .windows(b"WORKTREE_ALIVE".len())
            .any(|bytes| bytes == b"WORKTREE_ALIVE")
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Worktree切替後のPTYが入力に応答しません"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    tasks
        .set_worktree(&mut editor, connection.as_ref(), Some(&tree))
        .unwrap();
    assert_eq!(editor.active_document_id(), tree_document.as_ref());
    let reloaded = ConversationSession::new(repository, Default::default());
    assert!(reloaded.load_error().is_none());
    reloaded.restore_active(&mut editor).unwrap();
    assert_eq!(editor.active_text().unwrap(), "worktree draft");
    assert_eq!(
        std::fs::read_to_string(primary.join("note.ts")).unwrap(),
        "base"
    );
    assert_eq!(
        std::fs::read_to_string(tree_path.join("note.ts")).unwrap(),
        "base"
    );
    terminal.terminate(&id).unwrap();
}

#[test]
fn project_tasks_restore_primary_sub_drafts_and_keep_host_terminal_running() {
    let directory = tempfile::tempdir().unwrap();
    let primary = directory.path().join("primary");
    let sub = directory.path().join("sub");
    std::fs::create_dir(&primary).unwrap();
    std::fs::create_dir(&sub).unwrap();
    let primary = primary.canonicalize().unwrap();
    let sub = sub.canonicalize().unwrap();
    std::fs::write(primary.join("note.ts"), "primary").unwrap();
    std::fs::write(sub.join("note.ts"), "sub").unwrap();
    let host = BackendHost::start().unwrap();
    let mut editor = EditorSession::new_empty(
        host.workspaces(),
        Arc::new(NoDialog),
        Arc::new(LocalWorkspaceStateRepository::new(
            directory.path().join("editor.txt"),
        )),
        host.documents(),
    );
    editor.open_workspace(primary.clone()).unwrap();
    let repository = Arc::new(LocalConversationRepository::new(
        directory.path().join("tasks.json"),
    ));
    let mut tasks = ConversationSession::new(repository.clone(), editor.snapshot());
    let project = tasks.projects()[0].id().clone();
    tasks
        .register_source(&editor, &project, sub.clone())
        .unwrap();
    tasks
        .register_source(&editor, &project, primary.join("."))
        .unwrap();
    assert_eq!(tasks.projects()[0].sources().count(), 2);
    assert!(
        tasks
            .create_task(&mut editor, &project, directory.path().to_owned(), None)
            .is_err()
    );
    assert_eq!(editor.workspace_root(), Some(primary.as_path()));
    let first = tasks
        .create_task(&mut editor, &project, primary.clone(), Some(sub.clone()))
        .unwrap();
    editor.open_path(primary.join("note.ts")).unwrap();
    editor.replace_range(0..7, "primary draft").unwrap();
    let primary_document = editor.active_document_id().cloned();
    let mut terminal = TerminalSession::new(host.terminals());
    let terminal_id = terminal.start(&primary, 80, 24).unwrap();
    tasks
        .capture(&editor, Default::default(), None, terminal.terminals())
        .unwrap();
    tasks.switch_source(&mut editor, SourceRole::Sub).unwrap();
    assert_eq!(editor.workspace_root(), Some(sub.as_path()));
    assert!(tasks.active_record().unwrap().terminals.is_empty());
    editor.open_path(sub.join("note.ts")).unwrap();
    editor.replace_range(0..3, "sub draft").unwrap();
    tasks
        .capture(&editor, Default::default(), None, &[])
        .unwrap();
    let second = tasks
        .create_task(&mut editor, &project, primary.clone(), None)
        .unwrap();
    assert_ne!(first, second);
    assert!(editor.tabs().is_empty());
    tasks.switch(&first, &mut editor).unwrap();
    assert_eq!(editor.active_text().unwrap(), "sub draft");
    tasks
        .switch_source(&mut editor, SourceRole::Primary)
        .unwrap();
    assert_eq!(editor.active_document_id(), primary_document.as_ref());
    assert_eq!(editor.active_text().unwrap(), "primary draft");
    assert_eq!(tasks.active_record().unwrap().terminals.len(), 1);
    assert_eq!(tasks.active_record().unwrap().terminals[0].cwd, primary);
    terminal
        .input(
            &terminal_id,
            if cfg!(windows) {
                "Write-Output ('TASK_' + 'ALIVE')\r"
            } else {
                "printf 'TASK_%s\\n' ALIVE\r"
            },
        )
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        terminal.refresh().unwrap();
        if terminal.terminals()[0]
            .output
            .windows(b"TASK_ALIVE".len())
            .any(|bytes| bytes == b"TASK_ALIVE")
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Task切替後のPTYが入力に応答しません"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        std::fs::read_to_string(primary.join("note.ts")).unwrap(),
        "primary"
    );
    assert_eq!(std::fs::read_to_string(sub.join("note.ts")).unwrap(), "sub");
    let reloaded = ConversationSession::new(repository, Default::default());
    assert_eq!(reloaded.active_id(), &first);
    assert_eq!(reloaded.projects()[0].sources().count(), 2);
    reloaded.restore_active(&mut editor).unwrap();
    assert_eq!(editor.active_text().unwrap(), "primary draft");
    terminal.terminate(&terminal_id).unwrap();
}

#[test]
fn corrupt_catalog_is_reported_and_never_overwritten() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("tasks.json");
    std::fs::write(&path, "{broken").unwrap();
    let host = BackendHost::start().unwrap();
    let mut editor = EditorSession::new_empty(
        host.workspaces(),
        Arc::new(NoDialog),
        Arc::new(LocalWorkspaceStateRepository::new(
            directory.path().join("editor.txt"),
        )),
        host.documents(),
    );
    editor
        .open_workspace(directory.path().canonicalize().unwrap())
        .unwrap();
    let mut tasks = ConversationSession::new(
        Arc::new(LocalConversationRepository::new(path.clone())),
        editor.snapshot(),
    );
    assert!(tasks.load_error().is_some());
    assert!(
        tasks
            .capture(&editor, Default::default(), None, &[])
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), "{broken");
}

#[test]
fn failed_task_persistence_restores_the_previous_editor_and_task() {
    let directory = tempfile::tempdir().unwrap();
    let primary = directory.path().join("primary");
    std::fs::create_dir(&primary).unwrap();
    let primary = primary.canonicalize().unwrap();
    std::fs::write(primary.join("note.ts"), "original").unwrap();
    let host = BackendHost::start().unwrap();
    let mut editor = EditorSession::new_empty(
        host.workspaces(),
        Arc::new(NoDialog),
        Arc::new(LocalWorkspaceStateRepository::new(
            directory.path().join("editor.txt"),
        )),
        host.documents(),
    );
    editor.open_workspace(primary.clone()).unwrap();
    editor.open_path(primary.join("note.ts")).unwrap();
    editor.replace_range(0..8, "unsaved draft").unwrap();
    let blocked = directory.path().join("blocked");
    let mut tasks = ConversationSession::new(
        Arc::new(LocalConversationRepository::new(blocked.join("tasks.json"))),
        editor.snapshot(),
    );
    let active = tasks.active_id().clone();
    let project = tasks.projects()[0].id().clone();
    let document = editor.active_document_id().cloned();
    std::fs::write(blocked, "not a directory").unwrap();
    assert!(
        tasks
            .create_task(&mut editor, &project, primary, None)
            .is_err()
    );
    assert_eq!(tasks.active_id(), &active);
    assert_eq!(tasks.records().len(), 1);
    assert_eq!(editor.active_document_id(), document.as_ref());
    assert_eq!(editor.active_text().unwrap(), "unsaved draft");
}
