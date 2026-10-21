use lapis_git::{ConflictSide, GitAction, GitActionRequest, GitBackend};
use lapis_platform::LocalGitBackend;
use std::{path::Path, process::Command};

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}
fn setup() -> (tempfile::TempDir, LocalGitBackend) {
    let directory = tempfile::tempdir().unwrap();
    git(directory.path(), &["init", "-b", "main"]);
    git(directory.path(), &["config", "user.name", "Lapis Test"]);
    git(directory.path(), &["config", "core.autocrlf", "false"]);
    git(
        directory.path(),
        &["config", "user.email", "lapis@example.invalid"],
    );
    git(
        directory.path(),
        &[
            "config",
            "core.hooksPath",
            directory.path().join("no-hooks").to_str().unwrap(),
        ],
    );
    let backend = LocalGitBackend::new(
        directory.path().join(".test-state"),
        directory.path().join(".test-worktrees"),
    );
    (directory, backend)
}
fn act(backend: &LocalGitBackend, root: &Path, action: GitAction) -> String {
    let expected_head = action
        .is_mutation()
        .then(|| backend.status(root).unwrap().head);
    backend
        .execute(
            root,
            GitActionRequest {
                action,
                expected_head,
            },
        )
        .unwrap()
        .output
}
fn save(root: &Path, text: &str) {
    std::fs::write(root.join("note.txt"), text).unwrap();
}
fn commit(backend: &LocalGitBackend, root: &Path, text: &str) {
    act(
        backend,
        root,
        GitAction::Stage {
            path: "note.txt".into(),
        },
    );
    act(
        backend,
        root,
        GitAction::Commit {
            message: text.into(),
        },
    );
}

#[test]
fn first_commit_unstage_and_stale_head_are_handled_without_losing_working_files() {
    let (directory, backend) = setup();
    let root = directory.path();
    assert!(backend.status(root).unwrap().head.is_empty());
    save(root, "first\n");
    assert!(
        act(
            &backend,
            root,
            GitAction::Diff {
                path: "note.txt".into(),
                staged: false
            }
        )
        .contains("+first")
    );
    act(
        &backend,
        root,
        GitAction::Stage {
            path: "note.txt".into(),
        },
    );
    act(
        &backend,
        root,
        GitAction::Unstage {
            path: "note.txt".into(),
        },
    );
    assert!(root.join("note.txt").exists());
    commit(&backend, root, "first commit");
    assert!(act(&backend, root, GitAction::History).contains("first commit"));
    assert!(
        backend
            .execute(
                root,
                GitActionRequest {
                    action: GitAction::Commit {
                        message: "stale".into()
                    },
                    expected_head: Some(String::new())
                }
            )
            .is_err()
    );
    assert!(
        backend
            .execute(
                root,
                GitActionRequest {
                    action: GitAction::Stage {
                        path: "../outside".into()
                    },
                    expected_head: Some(backend.status(root).unwrap().head)
                }
            )
            .is_err()
    );
}

#[test]
fn individual_hunks_stage_unstage_and_reject_changed_diffs() {
    let (directory, backend) = setup();
    let root = directory.path();
    let base = (0..30)
        .map(|index| format!("line {index}\n"))
        .collect::<String>();
    save(root, &base);
    commit(&backend, root, "base");
    let changed = base
        .replace("line 2\n", "first change\n")
        .replace("line 25\n", "second change\n");
    save(root, &changed);
    let patch = act(
        &backend,
        root,
        GitAction::Diff {
            path: "note.txt".into(),
            staged: false,
        },
    );
    assert_eq!(
        patch.lines().filter(|line| line.starts_with("@@ ")).count(),
        2
    );
    act(
        &backend,
        root,
        GitAction::StageHunk {
            path: "note.txt".into(),
            staged: false,
            hunk: 0,
            expected_patch: patch.clone(),
        },
    );
    let cached = act(
        &backend,
        root,
        GitAction::Diff {
            path: "note.txt".into(),
            staged: true,
        },
    );
    assert!(cached.contains("+first change"));
    assert!(!cached.contains("+second change"));
    let status = backend.status(root).unwrap();
    assert_eq!(
        status
            .files
            .iter()
            .filter(|file| file.path == Path::new("note.txt"))
            .count(),
        2
    );
    assert!(
        backend
            .execute(
                root,
                GitActionRequest {
                    action: GitAction::StageHunk {
                        path: "note.txt".into(),
                        staged: false,
                        hunk: 1,
                        expected_patch: patch
                    },
                    expected_head: Some(status.head)
                }
            )
            .is_err()
    );
    act(
        &backend,
        root,
        GitAction::Stage {
            path: "note.txt".into(),
        },
    );
    let cached = act(
        &backend,
        root,
        GitAction::Diff {
            path: "note.txt".into(),
            staged: true,
        },
    );
    act(
        &backend,
        root,
        GitAction::StageHunk {
            path: "note.txt".into(),
            staged: true,
            hunk: 1,
            expected_patch: cached,
        },
    );
    let cached = act(
        &backend,
        root,
        GitAction::Diff {
            path: "note.txt".into(),
            staged: true,
        },
    );
    assert!(cached.contains("+first change"));
    assert!(!cached.contains("+second change"));
    assert_eq!(
        std::fs::read_to_string(root.join("note.txt")).unwrap(),
        changed
    );
}

#[test]
fn branch_stash_and_merge_conflicts_can_be_resolved_explicitly() {
    let (directory, backend) = setup();
    let root = directory.path();
    save(root, "base\n");
    commit(&backend, root, "base");
    act(
        &backend,
        root,
        GitAction::CreateBranch {
            name: "topic".into(),
        },
    );
    act(
        &backend,
        root,
        GitAction::SwitchBranch {
            name: "topic".into(),
        },
    );
    save(root, "topic\n");
    commit(&backend, root, "topic");
    act(
        &backend,
        root,
        GitAction::SwitchBranch {
            name: "main".into(),
        },
    );
    save(root, "stashed\n");
    act(
        &backend,
        root,
        GitAction::StashPush {
            message: "draft".into(),
        },
    );
    assert!(act(&backend, root, GitAction::Stashes).contains("draft"));
    act(
        &backend,
        root,
        GitAction::StashPop {
            index: 0,
            expected_oid: git(root, &["rev-parse", "stash@{0}"]).trim().to_owned(),
        },
    );
    assert_eq!(
        std::fs::read_to_string(root.join("note.txt")).unwrap(),
        "stashed\n"
    );
    commit(&backend, root, "main change");
    let request = GitActionRequest {
        action: GitAction::Merge {
            branch: "topic".into(),
        },
        expected_head: Some(backend.status(root).unwrap().head),
    };
    assert!(backend.execute(root, request).is_err());
    assert!(
        std::fs::read_to_string(root.join("note.txt"))
            .unwrap()
            .contains("<<<<<<<")
    );
    act(
        &backend,
        root,
        GitAction::Resolve {
            path: "note.txt".into(),
            side: ConflictSide::Theirs,
        },
    );
    assert_eq!(
        std::fs::read_to_string(root.join("note.txt")).unwrap(),
        "topic\n"
    );
    commit(&backend, root, "resolve merge");
    act(
        &backend,
        root,
        GitAction::DeleteBranch {
            name: "topic".into(),
        },
    );
    assert!(!act(&backend, root, GitAction::Branches).contains("topic"));
}

#[test]
fn literal_paths_do_not_stage_other_files_matching_git_pathspecs() {
    let (directory, backend) = setup();
    let root = directory.path();
    for name in ["[a].txt", "a.txt"] {
        std::fs::write(root.join(name), name).unwrap();
    }
    act(
        &backend,
        root,
        GitAction::Stage {
            path: "[a].txt".into(),
        },
    );
    let status = backend.status(root).unwrap();
    assert!(
        status
            .files
            .iter()
            .any(|file| file.path == Path::new("[a].txt") && file.staged)
    );
    assert!(
        status
            .files
            .iter()
            .any(|file| file.path == Path::new("a.txt") && !file.staged)
    );
    assert!(
        !status
            .files
            .iter()
            .any(|file| file.path == Path::new("a.txt") && file.staged)
    );
    act(
        &backend,
        root,
        GitAction::Unstage {
            path: "[a].txt".into(),
        },
    );
    assert!(
        backend
            .status(root)
            .unwrap()
            .files
            .iter()
            .all(|file| !file.staged)
    );
}

#[test]
fn stale_stash_selection_cannot_apply_pop_or_drop_a_different_stash() {
    let (directory, backend) = setup();
    let root = directory.path();
    save(root, "base\n");
    commit(&backend, root, "base");
    save(root, "first\n");
    act(
        &backend,
        root,
        GitAction::StashPush {
            message: "first".into(),
        },
    );
    let first = git(root, &["rev-parse", "stash@{0}"]).trim().to_owned();
    save(root, "second\n");
    act(
        &backend,
        root,
        GitAction::StashPush {
            message: "second".into(),
        },
    );
    let before = git(root, &["stash", "list", "--format=%H"]);
    for action in [
        GitAction::StashApply {
            index: 0,
            expected_oid: first.clone(),
        },
        GitAction::StashPop {
            index: 0,
            expected_oid: first.clone(),
        },
        GitAction::StashDrop {
            index: 0,
            expected_oid: first.clone(),
        },
    ] {
        assert!(
            backend
                .execute(
                    root,
                    GitActionRequest {
                        action,
                        expected_head: Some(backend.status(root).unwrap().head),
                    }
                )
                .is_err()
        );
        assert_eq!(git(root, &["stash", "list", "--format=%H"]), before);
        assert_eq!(
            std::fs::read_to_string(root.join("note.txt")).unwrap(),
            "base\n"
        );
    }
    act(
        &backend,
        root,
        GitAction::StashApply {
            index: 1,
            expected_oid: first,
        },
    );
    assert_eq!(
        std::fs::read_to_string(root.join("note.txt")).unwrap(),
        "first\n"
    );
    assert_eq!(git(root, &["stash", "list", "--format=%H"]), before);
}

#[test]
fn fetch_pull_and_push_use_the_selected_repository_and_configured_remote() {
    let (directory, backend) = setup();
    let root = directory.path();
    save(root, "base\n");
    commit(&backend, root, "base");
    let remote = tempfile::tempdir().unwrap();
    git(remote.path(), &["init", "--bare"]);
    git(
        root,
        &["remote", "add", "origin", remote.path().to_str().unwrap()],
    );
    act(&backend, root, GitAction::Push);
    git(remote.path(), &["symbolic-ref", "HEAD", "refs/heads/main"]);
    let peer_parent = tempfile::tempdir().unwrap();
    let peer = peer_parent.path().join("peer");
    git(
        peer_parent.path(),
        &[
            "clone",
            remote.path().to_str().unwrap(),
            peer.to_str().unwrap(),
        ],
    );
    git(&peer, &["config", "user.name", "Lapis Test"]);
    git(&peer, &["config", "user.email", "lapis@example.invalid"]);
    git(&peer, &["config", "core.autocrlf", "false"]);
    git(
        &peer,
        &[
            "config",
            "core.hooksPath",
            peer_parent.path().join("no-hooks").to_str().unwrap(),
        ],
    );
    save(&peer, "peer update\n");
    commit(&backend, &peer, "peer update");
    act(&backend, &peer, GitAction::Push);
    act(&backend, root, GitAction::Fetch);
    act(&backend, root, GitAction::Pull);
    assert_eq!(
        std::fs::read_to_string(root.join("note.txt")).unwrap(),
        "peer update\n"
    );
    save(root, "local change\n");
    commit(&backend, root, "local change");
    save(&peer, "diverged change\n");
    commit(&backend, &peer, "diverged change");
    act(&backend, &peer, GitAction::Push);
    let head = backend.status(root).unwrap().head;
    assert!(
        backend
            .execute(
                root,
                GitActionRequest {
                    action: GitAction::Pull,
                    expected_head: Some(head.clone())
                }
            )
            .is_err()
    );
    assert_eq!(backend.status(root).unwrap().head, head);
    assert_eq!(
        std::fs::read_to_string(root.join("note.txt")).unwrap(),
        "local change\n"
    );
}
