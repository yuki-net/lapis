use std::{path::Path, process::Command};

use lapis_client_api::{
    RequestBody, ResponseBody, WorkspaceOperation, WorkspaceOperationRequest,
    WorkspaceOperationResult,
};
use lapis_editor_core::WorkspaceId;
use lapis_host::BackendHost;

fn git(root: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn worktree_removal_preserves_detached_dirty_documents_and_running_terminals() {
    let root = tempfile::tempdir().unwrap();
    let sibling = tempfile::tempdir().unwrap();
    let tree = sibling.path().join("topic");
    git(root.path(), &["init", "-b", "main"]);
    git(root.path(), &["config", "user.name", "Lapis Test"]);
    git(
        root.path(),
        &["config", "user.email", "test@example.invalid"],
    );
    git(root.path(), &["config", "core.autocrlf", "false"]);
    git(
        root.path(),
        &[
            "config",
            "core.hooksPath",
            sibling.path().join("no-hooks").to_str().unwrap(),
        ],
    );
    std::fs::write(root.path().join("note.txt"), "base").unwrap();
    git(root.path(), &["add", "note.txt"]);
    git(root.path(), &["commit", "-m", "initial"]);
    git(
        root.path(),
        &["worktree", "add", "-b", "topic", tree.to_str().unwrap()],
    );
    let host = BackendHost::start().unwrap();
    let workspace = host.register_workspace(root.path()).unwrap();
    let workspace = WorkspaceId::new(workspace.as_str());
    let git = host.git();
    let id = git.worktrees(&workspace).unwrap().remove(0).worktree_id;
    let tree = git.open_worktree(&workspace, &id).unwrap();
    let documents = host.documents();
    let opened = documents.open(&tree.join("note.txt")).unwrap();
    let dirty = documents
        .edit(
            &opened.id,
            opened.document.revision(),
            0..4,
            "unsaved draft",
        )
        .unwrap();
    documents.close(&dirty.id).unwrap();
    drop(documents);
    assert!(git.discard_worktree(&workspace, &id).is_err());
    assert!(tree.exists());
    assert_eq!(
        std::fs::read_to_string(tree.join("note.txt")).unwrap(),
        "base"
    );
    let documents = host.documents();
    let restored = documents.attach(&dirty.id).unwrap();
    assert_eq!(restored.document.content_for_save(), "unsaved draft");
    documents
        .discard(&restored.id, restored.document.revision())
        .unwrap();
    documents.close(&restored.id).unwrap();
    drop(documents);
    let terminals = host.terminals();
    let terminal = terminals.start(&tree, 80, 24).unwrap();
    drop(terminals);
    assert!(git.discard_worktree(&workspace, &id).is_err());
    assert!(tree.exists());
    let reconnect = host.connect(&tree).unwrap();
    let response = reconnect
        .request(RequestBody::SnapshotResync(
            lapis_client_api::SnapshotRequest {
                workspace_id: reconnect.workspace_id().clone(),
                reason: lapis_client_api::SnapshotReason::Reconnect,
            },
        ))
        .unwrap();
    let ResponseBody::SnapshotResync(snapshot) = response else {
        panic!("snapshot required")
    };
    assert_eq!(snapshot.snapshot.terminals.len(), 1);
    assert_eq!(
        snapshot.snapshot.terminals[0].terminal_id.as_str(),
        terminal.as_str()
    );
    assert!(matches!(
        reconnect
            .request(RequestBody::TerminalTerminate(
                lapis_client_api::TerminalTerminateRequest {
                    terminal_id: snapshot.snapshot.terminals[0].terminal_id.clone(),
                }
            ))
            .unwrap(),
        ResponseBody::TerminalTerminate(_)
    ));
}

#[test]
fn desktop_and_wire_project_the_same_real_git_status_without_host_paths() {
    let root = tempfile::tempdir().unwrap();
    git(root.path(), &["init", "--initial-branch=main"]);
    git(root.path(), &["config", "user.name", "Lapis test"]);
    git(
        root.path(),
        &["config", "user.email", "test@example.invalid"],
    );
    std::fs::write(root.path().join("note.txt"), "before\n").unwrap();
    git(root.path(), &["add", "note.txt"]);
    git(root.path(), &["commit", "-m", "initial"]);
    std::fs::write(root.path().join("note.txt"), "after\n").unwrap();

    let host = BackendHost::start().unwrap();
    let workspace = host.register_workspace(root.path()).unwrap();
    let desktop = host.git();
    let snapshot = desktop
        .status(&WorkspaceId::new(workspace.as_str()), None)
        .unwrap();
    assert_eq!(snapshot.branch, "main");
    assert_eq!(snapshot.files.len(), 1);
    assert_eq!(snapshot.files[0].path, "note.txt");
    let diff = desktop
        .diff(&WorkspaceId::new(workspace.as_str()), None, "note.txt")
        .unwrap();
    assert!(diff.patch.contains("+after"));

    let wire = host.connect(root.path()).unwrap();
    let response = wire
        .request(RequestBody::WorkspaceExecute(WorkspaceOperationRequest {
            workspace_id: workspace,
            operation: WorkspaceOperation::GitStatus { worktree: None },
        }))
        .unwrap();
    assert_eq!(
        response,
        ResponseBody::WorkspaceExecute(WorkspaceOperationResult::GitStatus(snapshot))
    );
}

#[test]
fn desktop_and_wire_can_diff_tracked_files_after_their_parent_directory_is_deleted() {
    let root = tempfile::tempdir().unwrap();
    git(root.path(), &["init", "--initial-branch=main"]);
    git(root.path(), &["config", "user.name", "Lapis test"]);
    git(
        root.path(),
        &["config", "user.email", "test@example.invalid"],
    );
    std::fs::create_dir_all(root.path().join("removed/nested")).unwrap();
    std::fs::write(
        root.path().join("removed/nested/note.txt"),
        "tracked content\n",
    )
    .unwrap();
    git(root.path(), &["add", "removed/nested/note.txt"]);
    git(root.path(), &["commit", "-m", "initial"]);
    std::fs::remove_file(root.path().join("removed/nested/note.txt")).unwrap();
    std::fs::remove_dir(root.path().join("removed/nested")).unwrap();
    std::fs::remove_dir(root.path().join("removed")).unwrap();

    let host = BackendHost::start().unwrap();
    let workspace = host.register_workspace(root.path()).unwrap();
    let snapshot = host
        .git()
        .diff(
            &WorkspaceId::new(workspace.as_str()),
            None,
            "removed/nested/note.txt",
        )
        .unwrap();
    assert_eq!(snapshot.path, "removed/nested/note.txt");
    assert_eq!(snapshot.additions, 0);
    assert_eq!(snapshot.deletions, 1);
    assert!(snapshot.patch.contains("-tracked content"));

    let wire = host.connect(root.path()).unwrap();
    let response = wire
        .request(RequestBody::WorkspaceExecute(WorkspaceOperationRequest {
            workspace_id: workspace,
            operation: WorkspaceOperation::GitDiff {
                path: lapis_client_api::WorkspaceRelativePath::parse("removed/nested/note.txt")
                    .unwrap(),
                worktree: None,
            },
        }))
        .unwrap();
    assert_eq!(
        response,
        ResponseBody::WorkspaceExecute(WorkspaceOperationResult::GitDiff(snapshot))
    );
}

#[test]
fn stage_over_the_wire_changes_only_the_selected_workspace_index() {
    let roots = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
    for root in &roots {
        git(root.path(), &["init", "--initial-branch=main"]);
        std::fs::write(root.path().join("note.txt"), "draft\n").unwrap();
    }
    let host = BackendHost::start().unwrap();
    let first_id = host.register_workspace(roots[0].path()).unwrap();
    let second_id = host.register_workspace(roots[1].path()).unwrap();
    let wire = host.connect(roots[0].path()).unwrap();
    let request = lapis_git::GitActionRequest {
        action: lapis_git::GitAction::Stage {
            path: "note.txt".into(),
        },
        expected_head: Some(String::new()),
    };
    let denied = wire
        .request(RequestBody::WorkspaceExecute(WorkspaceOperationRequest {
            workspace_id: second_id.clone(),
            operation: WorkspaceOperation::GitAction {
                worktree: None,
                request: request.clone(),
            },
        }))
        .unwrap();
    assert!(
        matches!(denied, ResponseBody::Error(error) if error.code.as_str() == lapis_client_api::FORBIDDEN)
    );
    let response = wire
        .request(RequestBody::WorkspaceExecute(WorkspaceOperationRequest {
            workspace_id: first_id.clone(),
            operation: WorkspaceOperation::GitAction {
                worktree: None,
                request,
            },
        }))
        .unwrap();
    assert!(matches!(
        response,
        ResponseBody::WorkspaceExecute(WorkspaceOperationResult::GitAction(_))
    ));
    let connection = host.git();
    let first = connection
        .status(&WorkspaceId::new(first_id.as_str()), None)
        .unwrap();
    let second = connection
        .status(&WorkspaceId::new(second_id.as_str()), None)
        .unwrap();
    assert_eq!(first.files.len(), 1);
    assert_eq!(second.files.len(), 1);
    assert!(first.files[0].staged);
    assert!(!second.files[0].staged);
    for root in &roots {
        assert_eq!(
            std::fs::read_to_string(root.path().join("note.txt")).unwrap(),
            "draft\n"
        );
    }
}

#[test]
fn repository_discovery_projects_workspace_relative_paths() {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("frontend");
    let second = root.path().join("backend");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    git(&first, &["init", "-b", "main"]);
    git(&second, &["init", "-b", "main"]);
    let host = BackendHost::start().unwrap();
    let workspace = host.register_workspace(root.path()).unwrap();
    let wire = host.connect(root.path()).unwrap();
    let response = wire
        .request(RequestBody::WorkspaceExecute(WorkspaceOperationRequest {
            workspace_id: workspace.clone(),
            operation: WorkspaceOperation::GitRepositories,
        }))
        .unwrap();
    let ResponseBody::WorkspaceExecute(WorkspaceOperationResult::GitRepositories(repositories)) =
        response
    else {
        panic!("repository list required");
    };
    assert_eq!(repositories.len(), 2);
    assert_ne!(repositories[0].id, repositories[1].id);
    assert_eq!(
        repositories
            .iter()
            .map(|repo| repo.path.as_deref().unwrap())
            .collect::<Vec<_>>(),
        vec!["backend", "frontend"]
    );
    let desktop = host.git();
    assert_eq!(
        desktop
            .repositories(&WorkspaceId::new(workspace.as_str()))
            .unwrap(),
        repositories
    );
    let serialized = serde_json::to_string(&repositories).unwrap();
    assert!(!serialized.contains(&root.path().to_string_lossy().replace('\\', "\\\\")));
}

#[test]
fn repository_operations_are_scoped_to_the_selected_repository() {
    let root = tempfile::tempdir().unwrap();
    for name in ["frontend", "backend"] {
        let path = root.path().join(name);
        std::fs::create_dir(&path).unwrap();
        git(&path, &["init", "-b", "main"]);
        std::fs::write(path.join("note.txt"), "draft\n").unwrap();
    }
    let host = BackendHost::start().unwrap();
    let workspace = host.register_workspace(root.path()).unwrap();
    let wire = host.connect(root.path()).unwrap();
    let execute = |operation| {
        wire.request(RequestBody::WorkspaceExecute(WorkspaceOperationRequest {
            workspace_id: workspace.clone(),
            operation,
        }))
        .unwrap()
    };
    let response = execute(WorkspaceOperation::RepositoryGitAction {
        repository_id: "path:frontend".into(),
        request: lapis_git::GitActionRequest {
            action: lapis_git::GitAction::Stage {
                path: "note.txt".into(),
            },
            expected_head: Some(String::new()),
        },
    });
    assert!(matches!(
        response,
        ResponseBody::WorkspaceExecute(WorkspaceOperationResult::GitAction(_))
    ));
    for (name, staged) in [("frontend", true), ("backend", false)] {
        let response = execute(WorkspaceOperation::RepositoryGitStatus {
            repository_id: format!("path:{name}"),
        });
        let ResponseBody::WorkspaceExecute(WorkspaceOperationResult::RepositoryGitStatus {
            status,
            ..
        }) = response
        else {
            panic!("status required");
        };
        assert_eq!(status.files.len(), 1);
        assert_eq!(status.files[0].staged, staged);
    }
    let response = execute(WorkspaceOperation::RepositoryGitStatus {
        repository_id: "path:../outside".into(),
    });
    assert!(matches!(response, ResponseBody::Error(_)));
}

#[test]
fn repository_status_cache_observes_external_files_and_git_index() {
    use std::time::{Duration, Instant};
    let root = tempfile::tempdir().unwrap();
    git(root.path(), &["init", "-b", "main"]);
    let host = BackendHost::start().unwrap();
    let workspace = host.register_workspace(root.path()).unwrap();
    let workspace = WorkspaceId::new(workspace.as_str());
    let git_client = host.git();
    assert!(
        git_client
            .repository_status(&workspace, "root")
            .unwrap()
            .files
            .is_empty()
    );
    std::fs::write(root.path().join("outside.txt"), "external\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let snapshot = git_client.repository_status(&workspace, "root").unwrap();
        if snapshot
            .files
            .iter()
            .any(|file| file.path == "outside.txt" && !file.staged)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "external file change was not observed"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    git(root.path(), &["add", "outside.txt"]);
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let snapshot = git_client.repository_status(&workspace, "root").unwrap();
        if snapshot
            .files
            .iter()
            .any(|file| file.path == "outside.txt" && file.staged)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "external index change was not observed"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn worktree_cache_observes_git_metadata_outside_workspace() {
    use std::time::{Duration, Instant};
    let repository = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let tree = parent.path().join("tree");
    git(repository.path(), &["init", "-b", "main"]);
    git(repository.path(), &["config", "user.name", "Lapis Test"]);
    git(
        repository.path(),
        &["config", "user.email", "test@example.invalid"],
    );
    std::fs::write(repository.path().join("file.txt"), "base\n").unwrap();
    git(repository.path(), &["add", "file.txt"]);
    git(repository.path(), &["commit", "-m", "initial"]);
    git(
        repository.path(),
        &["worktree", "add", "--detach", tree.to_str().unwrap()],
    );
    let host = BackendHost::start().unwrap();
    let workspace = host.register_workspace(&tree).unwrap();
    let workspace = WorkspaceId::new(workspace.as_str());
    let client = host.git();
    assert!(
        client
            .repository_status(&workspace, "root")
            .unwrap()
            .files
            .is_empty()
    );
    std::fs::write(tree.join("file.txt"), "changed\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let snapshot = client.repository_status(&workspace, "root").unwrap();
        if snapshot
            .files
            .iter()
            .any(|file| file.path == "file.txt" && !file.staged)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "external worktree file change was not observed"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    git(&tree, &["add", "file.txt"]);
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let snapshot = client.repository_status(&workspace, "root").unwrap();
        if snapshot
            .files
            .iter()
            .any(|file| file.path == "file.txt" && file.staged)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "external worktree index change was not observed"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn external_git_changes_are_pushed_without_status_requests() {
    use std::time::{Duration, Instant};
    let root = tempfile::tempdir().unwrap();
    git(root.path(), &["init", "-b", "main"]);
    git(root.path(), &["config", "user.name", "Lapis Test"]);
    git(
        root.path(),
        &["config", "user.email", "test@example.invalid"],
    );
    std::fs::write(root.path().join("file.txt"), "base\n").unwrap();
    git(root.path(), &["add", "file.txt"]);
    git(root.path(), &["commit", "-m", "initial"]);
    let host = BackendHost::start().unwrap();
    let workspace = host.register_workspace(root.path()).unwrap();
    let wire = host.connect(root.path()).unwrap();
    let events = wire.subscribe().unwrap();
    let response = wire
        .request(RequestBody::WorkspaceExecute(WorkspaceOperationRequest {
            workspace_id: workspace.clone(),
            operation: WorkspaceOperation::RepositoryGitStatus {
                repository_id: "root".into(),
            },
        }))
        .unwrap();
    let ResponseBody::WorkspaceExecute(WorkspaceOperationResult::RepositoryGitStatus {
        status,
        ..
    }) = response
    else {
        panic!("status required");
    };
    let revision = status.revision;
    std::fs::write(root.path().join("file.txt"), "external\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let event = events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("Git event missing");
        if let lapis_client_api::EventBody::GitStatusChanged { repository, .. } = event.body
            && repository.status.revision > revision
            && repository
                .status
                .files
                .iter()
                .any(|file| file.path == "file.txt" && !file.staged)
        {
            break;
        }
    }
    git(root.path(), &["add", "file.txt"]);
    loop {
        let event = events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("stage event missing");
        if let lapis_client_api::EventBody::GitStatusChanged { repository, .. } = event.body
            && repository
                .status
                .files
                .iter()
                .any(|file| file.path == "file.txt" && file.staged)
        {
            break;
        }
    }
    git(root.path(), &["commit", "-m", "external"]);
    git(root.path(), &["switch", "-c", "topic"]);
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let event = events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("branch event missing");
        if let lapis_client_api::EventBody::GitStatusChanged { repository, .. } = event.body
            && repository.status.branch == "topic"
            && repository.status.head != status.head
        {
            break;
        }
    }
    let reconnect = host.connect(root.path()).unwrap();
    let response = reconnect
        .request(RequestBody::SnapshotResync(
            lapis_client_api::SnapshotRequest {
                workspace_id: workspace,
                reason: lapis_client_api::SnapshotReason::Reconnect,
            },
        ))
        .unwrap();
    let ResponseBody::SnapshotResync(response) = response else {
        panic!("snapshot required")
    };
    assert!(
        response
            .snapshot
            .git_repositories
            .iter()
            .any(|item| item.repository_id == "root" && item.status.branch == "topic")
    );
}

#[test]
fn git_subscription_receives_updates_and_reconnects_to_snapshot() {
    use std::time::{Duration, Instant};
    let root = tempfile::tempdir().unwrap();
    git(root.path(), &["init", "-b", "main"]);
    let host = BackendHost::start().unwrap();
    let workspace = host.register_workspace(root.path()).unwrap();
    let workspace = WorkspaceId::new(workspace.as_str());
    let client = host.git();
    let mut events = client.subscribe(&workspace).unwrap();
    assert!(matches!(
        events.recv_timeout(Duration::from_secs(2)).unwrap(),
        Some(lapis_git::GitUpdate::Snapshot(_))
    ));
    client.repository_status(&workspace, "root").unwrap();
    std::fs::write(root.path().join("file.txt"), "changed\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(lapis_git::GitUpdate::Changed(status)) = events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap()
            && status
                .status
                .files
                .iter()
                .any(|file| file.path == "file.txt")
        {
            break;
        }
        assert!(Instant::now() < deadline);
    }
    drop(events);
    let mut reconnected = client.subscribe(&workspace).unwrap();
    let Some(lapis_git::GitUpdate::Snapshot(snapshot)) =
        reconnected.recv_timeout(Duration::from_secs(2)).unwrap()
    else {
        panic!("Git snapshot required")
    };
    assert!(snapshot.iter().any(|entry| {
        entry.repository_id == "root"
            && entry
                .status
                .files
                .iter()
                .any(|file| file.path == "file.txt")
    }));
}

#[test]
fn repository_rescan_removes_status_from_reconnect_snapshot() {
    use std::time::{Duration, Instant};
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("frontend");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    let host = BackendHost::start().unwrap();
    let workspace = WorkspaceId::new(host.register_workspace(root.path()).unwrap().as_str());
    let client = host.git();
    assert_eq!(client.repositories(&workspace).unwrap().len(), 1);
    let mut events = client.subscribe(&workspace).unwrap();
    events.recv_timeout(Duration::from_secs(2)).unwrap();
    client
        .repository_status(&workspace, "path:frontend")
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if matches!(
            events.recv_timeout(Duration::from_secs(1)).unwrap(),
            Some(lapis_git::GitUpdate::Changed(_))
        ) {
            break;
        }
        assert!(Instant::now() < deadline);
    }
    std::fs::rename(repo.join(".git"), root.path().join("removed-git")).unwrap();
    assert!(client.repositories(&workspace).unwrap().is_empty());
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if matches!(
            events.recv_timeout(Duration::from_secs(1)).unwrap(),
            Some(lapis_git::GitUpdate::RepositoriesChanged)
        ) {
            break;
        }
        assert!(Instant::now() < deadline);
    }
    let mut reconnected = client.subscribe(&workspace).unwrap();
    assert!(
        matches!(reconnected.recv_timeout(Duration::from_secs(2)).unwrap(), Some(lapis_git::GitUpdate::Snapshot(statuses)) if statuses.is_empty())
    );
}

#[test]
fn worktree_events_and_reconnect_snapshot_follow_creation_and_removal() {
    use std::time::{Duration, Instant};
    let root = tempfile::tempdir().unwrap();
    git(root.path(), &["init", "-b", "main"]);
    git(root.path(), &["config", "user.name", "Lapis Test"]);
    git(
        root.path(),
        &["config", "user.email", "test@example.invalid"],
    );
    std::fs::write(root.path().join("file"), "initial").unwrap();
    git(root.path(), &["add", "file"]);
    git(root.path(), &["commit", "-m", "initial"]);
    let host = BackendHost::start().unwrap();
    let workspace = WorkspaceId::new(host.register_workspace(root.path()).unwrap().as_str());
    let client_a = host.git();
    let client_b = host.git();
    let mut events = client_b.subscribe(&workspace).unwrap();
    events.recv_timeout(Duration::from_secs(2)).unwrap();
    let tree = client_a.create_worktree(&workspace).unwrap();
    let wait_for_change = |events: &mut Box<dyn lapis_git::GitSubscription>| {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if matches!(
                events.recv_timeout(Duration::from_secs(1)).unwrap(),
                Some(lapis_git::GitUpdate::WorktreesChanged)
            ) {
                break;
            }
            assert!(Instant::now() < deadline);
        }
    };
    wait_for_change(&mut events);
    assert!(
        client_b
            .worktrees(&workspace)
            .unwrap()
            .iter()
            .any(|item| item.worktree_id == tree.worktree_id)
    );
    client_b
        .status(&workspace, Some(&tree.worktree_id))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if matches!(events.recv_timeout(Duration::from_secs(1)).unwrap(), Some(lapis_git::GitUpdate::Changed(item)) if item.status.worktree_id == Some(tree.worktree_id.clone()))
        {
            break;
        }
        assert!(Instant::now() < deadline);
    }
    client_a
        .discard_worktree(&workspace, &tree.worktree_id)
        .unwrap();
    wait_for_change(&mut events);
    assert!(
        client_b
            .worktrees(&workspace)
            .unwrap()
            .iter()
            .all(|item| item.worktree_id != tree.worktree_id
                || item.state == lapis_git::WorktreeState::Discarded)
    );
    let mut reconnected = client_b.subscribe(&workspace).unwrap();
    assert!(
        matches!(reconnected.recv_timeout(Duration::from_secs(2)).unwrap(), Some(lapis_git::GitUpdate::Snapshot(statuses)) if statuses.iter().all(|item| item.status.worktree_id != Some(tree.worktree_id.clone())))
    );
}
