use lapis_desktop_client::{
    ConversationSession, EditorSession, SourceRole, TerminalSession, WorkspaceDialog,
};
use lapis_host::BackendHost;
use lapis_persistence::{LocalConversationRepository, LocalWorkspaceStateRepository};
use std::{path::PathBuf, sync::Arc};

struct NoDialog;

#[test]
fn concurrent_task_saves_preserve_both_drafts_and_allow_further_saves() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    for name in ["a.ts", "b.ts"] {
        std::fs::write(root.join(name), "base").unwrap();
    }
    let host = BackendHost::start().unwrap();
    let make_editor = |name: &str| {
        let mut editor = EditorSession::new_empty(
            host.workspaces(),
            Arc::new(NoDialog),
            Arc::new(LocalWorkspaceStateRepository::new(root.join(name))),
            host.documents(),
        );
        editor.open_workspace(root.clone()).unwrap();
        editor
    };
    let mut first_editor = make_editor("first-editor.json");
    let mut second_editor = make_editor("second-editor.json");
    let repository = Arc::new(LocalConversationRepository::new(root.join("tasks.json")));
    let mut first = ConversationSession::new(repository.clone(), first_editor.snapshot());
    first
        .capture(&first_editor, Default::default(), None, &[])
        .unwrap();
    let mut second = ConversationSession::new(repository.clone(), second_editor.snapshot());
    first_editor.open_path(root.join("a.ts")).unwrap();
    first_editor.replace_range(0..4, "first draft").unwrap();
    second_editor.open_path(root.join("b.ts")).unwrap();
    second_editor.replace_range(0..4, "second draft").unwrap();
    first
        .capture(&first_editor, Default::default(), None, &[])
        .unwrap();
    second
        .capture(&second_editor, Default::default(), None, &[])
        .unwrap();
    assert_ne!(first.active_id(), second.active_id());
    assert!(
        second
            .active_record()
            .unwrap()
            .title
            .contains("競合から復旧")
    );
    let count = second.records().len();
    second
        .capture(&second_editor, Default::default(), None, &[])
        .unwrap();
    assert_eq!(second.records().len(), count);
    let reloaded = ConversationSession::new(repository, Default::default());
    assert!(reloaded.load_error().is_none());
    let drafts: Vec<_> = reloaded
        .records()
        .iter()
        .flat_map(|record| record.workspace.open_documents.iter())
        .filter_map(|document| document.draft_content.as_deref())
        .collect();
    assert!(drafts.contains(&"first draft"));
    assert!(drafts.contains(&"second draft"));
    let first_root = root.join("first-project");
    let second_root = root.join("second-project");
    std::fs::create_dir(&first_root).unwrap();
    std::fs::create_dir(&second_root).unwrap();
    let first_project = first
        .register_project(&first_editor, first_root.clone())
        .unwrap();
    let second_project = second
        .register_project(&second_editor, second_root.clone())
        .unwrap();
    assert_ne!(first_project, second_project);
    second
        .create_task(&mut second_editor, &second_project, second_root, None)
        .unwrap();
    assert!(
        second
            .projects()
            .iter()
            .any(|project| project.root() == first_root)
    );
}

#[test]
fn closing_workspace_uses_a_scratch_record_and_keeps_the_task_draft() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    std::fs::write(root.join("note.ts"), "base").unwrap();
    let host = BackendHost::start().unwrap();
    let mut editor = EditorSession::new_empty(
        host.workspaces(),
        Arc::new(NoDialog),
        Arc::new(LocalWorkspaceStateRepository::new(root.join("editor.json"))),
        host.documents(),
    );
    editor.open_workspace(root.clone()).unwrap();
    let repository = Arc::new(LocalConversationRepository::new(root.join("tasks.json")));
    let mut tasks = ConversationSession::new(repository.clone(), editor.snapshot());
    editor.open_path(root.join("note.ts")).unwrap();
    editor.replace_range(0..4, "draft").unwrap();
    tasks
        .capture(&editor, Default::default(), None, &[])
        .unwrap();
    let original = tasks.active_id().clone();
    tasks.close_workspace(&mut editor).unwrap();
    assert!(editor.workspace_root().is_none());
    assert!(tasks.active_record().unwrap().environment.is_none());
    tasks
        .capture(&editor, Default::default(), None, &[])
        .unwrap();
    tasks.switch(&original, &mut editor).unwrap();
    assert_eq!(editor.active_text().unwrap(), "draft");
    assert!(
        ConversationSession::new(repository, Default::default())
            .load_error()
            .is_none()
    );
}
impl WorkspaceDialog for NoDialog {
    fn choose_workspace_path(&self) -> Option<PathBuf> {
        None
    }
    fn choose_save_path(&self, _: &str) -> Option<PathBuf> {
        None
    }
}

#[test]
fn reopening_project_restores_inactive_task_without_creating_duplicates() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let other = root.join("other");
    std::fs::create_dir(&other).unwrap();
    std::fs::write(root.join("note.ts"), "original").unwrap();
    let host = BackendHost::start().unwrap();
    let mut editor = EditorSession::new_empty(
        host.workspaces(),
        Arc::new(NoDialog),
        Arc::new(LocalWorkspaceStateRepository::new(root.join("editor.txt"))),
        host.documents(),
    );
    editor.open_workspace(root.clone()).unwrap();
    let mut tasks = ConversationSession::new(
        Arc::new(LocalConversationRepository::new(root.join("tasks.json"))),
        editor.snapshot(),
    );
    let first = tasks.active_id().clone();
    let project = tasks.projects()[0].id().clone();
    editor.open_path(root.join("note.ts")).unwrap();
    editor.replace_range(0..8, "draft").unwrap();
    tasks
        .capture(&editor, Default::default(), None, &[])
        .unwrap();
    tasks.open_project(&mut editor, other).unwrap();
    let count = tasks.records().len();
    tasks.open_project(&mut editor, root.clone()).unwrap();
    assert_eq!(tasks.active_id(), &first);
    assert_eq!(tasks.records().len(), count);
    assert_eq!(editor.active_text().unwrap(), "draft");
    tasks
        .create_task(&mut editor, &project, root.clone(), None)
        .unwrap();
    let active = tasks.active_id().clone();
    let before = editor.snapshot();
    assert!(tasks.open_project(&mut editor, root).is_err());
    assert_eq!(tasks.active_id(), &active);
    assert_eq!(tasks.records().len(), count + 1);
    assert_eq!(editor.snapshot(), before);
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
