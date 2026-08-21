use std::fs;

use lapis_backend_state::{PathSecurityError, WorkspacePathResolver};
use lapis_client_api::WorkspaceRelativePath;

#[test]
fn existing_and_new_paths_remain_inside_workspace() {
    let workspace = tempfile::tempdir().unwrap();
    fs::create_dir(workspace.path().join("src")).unwrap();
    fs::write(workspace.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    let resolver = WorkspacePathResolver::new(workspace.path().to_owned()).unwrap();

    let existing = resolver
        .resolve_existing(&WorkspaceRelativePath::parse("src/main.rs").unwrap())
        .unwrap();
    let new_file = resolver
        .resolve_new_file(&WorkspaceRelativePath::parse("src/new.rs").unwrap())
        .unwrap();

    assert!(existing.starts_with(resolver.root()));
    assert!(new_file.starts_with(resolver.root()));
}

#[test]
fn symlink_escape_is_rejected_when_platform_allows_symlinks() {
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let outside_file = outside.path().join("secret.txt");
    fs::write(&outside_file, "secret").unwrap();
    let link = workspace.path().join("linked-secret.txt");

    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside_file, &link).unwrap();
    #[cfg(windows)]
    if std::os::windows::fs::symlink_file(&outside_file, &link).is_err() {
        return;
    }

    let resolver = WorkspacePathResolver::new(workspace.path().to_owned()).unwrap();
    assert!(matches!(
        resolver.resolve_existing(&WorkspaceRelativePath::parse("linked-secret.txt").unwrap()),
        Err(PathSecurityError::WorkspaceEscape)
    ));
}

#[test]
fn dangling_symlink_cannot_be_reused_as_new_file() {
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let outside_file = outside.path().join("not-created.txt");
    let link = workspace.path().join("new.txt");

    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside_file, &link).unwrap();
    #[cfg(windows)]
    if std::os::windows::fs::symlink_file(&outside_file, &link).is_err() {
        return;
    }

    let resolver = WorkspacePathResolver::new(workspace.path().to_owned()).unwrap();
    assert!(
        resolver
            .resolve_new_file(&WorkspaceRelativePath::parse("new.txt").unwrap())
            .is_err()
    );
}
