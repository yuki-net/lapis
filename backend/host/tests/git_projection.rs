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
