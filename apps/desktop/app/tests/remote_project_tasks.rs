use lapis_client_api::*;
use lapis_desktop_client::{
    ConversationSession, EditorSession, SourceRole, TerminalSession, WorkspaceDialog,
};
use lapis_host::{BackendClient, BackendHost, HostRemoteHandler};
use lapis_persistence::{LocalConversationRepository, LocalWorkspaceStateRepository};
use lapis_remote::*;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

struct NoDialog;
impl WorkspaceDialog for NoDialog {
    fn choose_workspace_path(&self) -> Option<PathBuf> {
        None
    }
    fn choose_save_path(&self, _: &str) -> Option<PathBuf> {
        None
    }
}
fn git(root: &Path, args: &[&str]) {
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
fn editor(backend: &BackendClient, state: &Path) -> EditorSession {
    EditorSession::new_empty(
        backend.workspaces(),
        Arc::new(NoDialog),
        Arc::new(LocalWorkspaceStateRepository::new(state.to_owned())),
        backend.documents(),
    )
}

#[test]
fn quic_project_tasks_restore_primary_sub_worktree_drafts_and_live_terminal() {
    let directory = tempfile::tempdir().unwrap();
    let primary = directory.path().join("primary");
    let sub = directory.path().join("sub");
    let tree = directory.path().join("topic");
    std::fs::create_dir(&primary).unwrap();
    std::fs::write(primary.join("note.ts"), "base").unwrap();
    git(&primary, &["init", "-b", "main"]);
    git(&primary, &["config", "user.name", "Lapis Test"]);
    git(&primary, &["config", "user.email", "test@example.invalid"]);
    git(&primary, &["config", "core.autocrlf", "false"]);
    git(
        &primary,
        &[
            "config",
            "core.hooksPath",
            directory.path().join("no-hooks").to_str().unwrap(),
        ],
    );
    git(&primary, &["add", "note.ts"]);
    git(&primary, &["commit", "-m", "initial"]);
    git(
        &primary,
        &["worktree", "add", "-b", "sub", sub.to_str().unwrap()],
    );
    std::fs::write(sub.join("note.ts"), "sub").unwrap();
    git(
        &primary,
        &["worktree", "add", "-b", "topic", tree.to_str().unwrap()],
    );
    let host = BackendHost::start().unwrap();
    let primary_id = host.register_workspace(&primary).unwrap();
    let sub_id = host.register_workspace(&sub).unwrap();
    let capabilities = lapis_host::supported_capabilities();
    let auth = Arc::new(Mutex::new(RemoteAuth::system(
        AuthConfig::new(
            PairingLifetime::new(300).unwrap(),
            CredentialLifetime::new(3600).unwrap(),
        ),
        AuthPolicy::new(capabilities.clone()),
    )));
    let client_id = ClientId::try_new("project-task-integration").unwrap();
    let credential = {
        let mut auth = auth.lock().unwrap();
        auth.enable();
        let token = auth
            .begin_project_pairing(primary_id.clone(), [sub_id])
            .unwrap();
        Arc::new(
            auth.complete_pairing(&token, &client_id, &capabilities)
                .unwrap(),
        )
    };
    let certificate = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let tls = tls13_server_config(
        vec![certificate.cert.der().clone()],
        rustls::pki_types::PrivateKeyDer::Pkcs8(certificate.signing_key.serialize_der().into()),
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let server = runtime
        .block_on(QuicServer::bind(
            "127.0.0.1:0".parse().unwrap(),
            tls,
            auth.clone(),
            Arc::new(HostRemoteHandler::new(host).with_auth(auth)),
            RemoteLimits::default(),
        ))
        .unwrap();
    let config = QuicConnectionConfig {
        address: server.listening_address().unwrap(),
        server_name: "localhost".into(),
        certificate: certificate.cert.der().clone(),
        workspace_id: primary_id,
        credential,
        hello: ClientHello {
            protocol: ProtocolRange::exact(CURRENT_PROTOCOL),
            client_id,
            client_name: "Test".into(),
            client_kind: ClientKind::Desktop,
            requested_capabilities: capabilities,
        },
    };
    let virtual_root = directory.path().join("client-namespace");
    let backend = BackendClient::remote(virtual_root.clone(), config.clone()).unwrap();
    let catalog = backend.workspaces().available_sources().unwrap();
    let virtual_sub = catalog
        .into_iter()
        .find(|(root, _)| root != &virtual_root)
        .unwrap()
        .0;
    let mut editor = editor(&backend, &directory.path().join("editor.json"));
    editor.open_workspace(virtual_root.clone()).unwrap();
    let repository = Arc::new(LocalConversationRepository::new(
        directory.path().join("tasks.json"),
    ));
    let mut tasks = ConversationSession::new(repository.clone(), editor.snapshot());
    let project = tasks.projects()[0].id().clone();
    tasks
        .register_source(&editor, &project, virtual_sub.clone())
        .unwrap();
    let task = tasks
        .create_task(
            &mut editor,
            &project,
            virtual_root.clone(),
            Some(virtual_sub),
        )
        .unwrap();
    let git = backend.git();
    let trees = git.worktrees(editor.workspace_id().unwrap()).unwrap();
    let find = |branch: &str| {
        trees
            .iter()
            .find(|tree| {
                git.status(editor.workspace_id().unwrap(), Some(&tree.worktree_id))
                    .unwrap()
                    .branch
                    == branch
            })
            .unwrap()
            .worktree_id
            .clone()
    };
    let worktree = find("topic");
    let sub_worktree = find("sub");
    assert!(
        tasks
            .set_worktree(&mut editor, git.as_ref(), Some(&sub_worktree))
            .is_err()
    );
    assert_eq!(editor.workspace_root(), Some(virtual_root.as_path()));
    editor.open_path(virtual_root.join("note.ts")).unwrap();
    editor.replace_range(0..4, "primary draft").unwrap();
    let primary_document = editor.active_document_id().cloned();
    let mut terminals = TerminalSession::new(backend.terminals());
    let terminal = terminals.start(&virtual_root, 80, 24).unwrap();
    tasks
        .capture(&editor, Default::default(), None, terminals.terminals())
        .unwrap();
    tasks
        .set_worktree(&mut editor, git.as_ref(), Some(&worktree))
        .unwrap();
    let virtual_tree = editor.workspace_root().unwrap().to_owned();
    assert!(virtual_tree.starts_with(&virtual_root) && !virtual_tree.exists());
    editor.open_path(virtual_tree.join("note.ts")).unwrap();
    editor.replace_range(0..4, "worktree draft").unwrap();
    let worktree_document = editor.active_document_id().cloned();
    tasks
        .capture(&editor, Default::default(), None, &[])
        .unwrap();
    tasks.switch_source(&mut editor, SourceRole::Sub).unwrap();
    assert!(editor.workspace_path(&virtual_root, "note.ts").is_err());
    assert!(editor.workspace_path(&virtual_tree, "note.ts").is_err());
    let sub_root = editor.workspace_root().unwrap().to_owned();
    assert_eq!(
        editor.workspace_path(&sub_root, "note.ts").unwrap(),
        sub_root.join("note.ts")
    );
    assert!(editor.workspace_path(&sub_root, "../note.ts").is_err());
    editor
        .open_path(editor.workspace_root().unwrap().join("note.ts"))
        .unwrap();
    editor.replace_range(0..3, "sub draft").unwrap();
    tasks
        .capture(&editor, Default::default(), None, &[])
        .unwrap();
    tasks
        .create_task(&mut editor, &project, virtual_root.clone(), None)
        .unwrap();
    assert!(editor.tabs().is_empty());
    tasks.switch(&task, &mut editor).unwrap();
    assert_eq!(editor.active_text().unwrap(), "sub draft");
    tasks
        .switch_source(&mut editor, SourceRole::Primary)
        .unwrap();
    assert_eq!(editor.active_document_id(), worktree_document.as_ref());
    assert_eq!(editor.active_text().unwrap(), "worktree draft");
    tasks.set_worktree(&mut editor, git.as_ref(), None).unwrap();
    assert_eq!(editor.active_document_id(), primary_document.as_ref());
    assert_eq!(editor.active_text().unwrap(), "primary draft");
    assert_eq!(
        tasks.active_record().unwrap().terminals[0].cwd,
        virtual_root
    );
    terminals
        .input(
            &terminal,
            if cfg!(windows) {
                "Write-Output ('QUIC_TASK_' + 'ALIVE')\r"
            } else {
                "printf 'QUIC_TASK_%s\\n' ALIVE\r"
            },
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        terminals.refresh().unwrap();
        if terminals.terminals()[0]
            .output
            .windows(b"QUIC_TASK_ALIVE".len())
            .any(|bytes| bytes == b"QUIC_TASK_ALIVE")
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Remote Task切替後のPTYが応答しません"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    terminals.terminate(&terminal).unwrap();
    tasks
        .set_worktree(&mut editor, git.as_ref(), Some(&worktree))
        .unwrap();
    drop(editor);
    let reloaded = ConversationSession::new(repository, Default::default());
    assert!(reloaded.load_error().is_none());
    let fresh = BackendClient::remote(virtual_root, config).unwrap();
    let mut fresh_editor = self::editor(&fresh, &directory.path().join("fresh.json"));
    reloaded.restore_active(&mut fresh_editor).unwrap();
    assert_eq!(
        fresh_editor.active_document_id(),
        worktree_document.as_ref()
    );
    assert_eq!(fresh_editor.active_text().unwrap(), "worktree draft");
    assert_eq!(
        std::fs::read_to_string(primary.join("note.ts")).unwrap(),
        "base"
    );
    assert_eq!(
        std::fs::read_to_string(tree.join("note.ts")).unwrap(),
        "base"
    );
    assert_eq!(std::fs::read_to_string(sub.join("note.ts")).unwrap(), "sub");
}
