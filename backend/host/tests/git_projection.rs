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
