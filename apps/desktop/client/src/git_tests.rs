#[derive(Default)]
struct GitFixture {
    discoveries: Mutex<usize>,
    actions: Mutex<Vec<String>>,
    failing_repository: Mutex<Option<String>>,
    fail_discovery: Mutex<bool>,
    revisions: Mutex<std::collections::HashMap<String, u64>>,
}

impl lapis_git::GitConnection for GitFixture {
    fn repositories(&self, _: &WorkspaceId) -> Result<Vec<lapis_git::GitRepositorySnapshot>, GitError> {
        *self.discoveries.lock().unwrap() += 1;
        if *self.fail_discovery.lock().unwrap() {
            return Err(GitError::new(
                lapis_git::GitErrorKind::Io,
                "repository discovery failed",
            ));
        }
        Ok(["frontend", "backend"].into_iter().map(|name| lapis_git::GitRepositorySnapshot { id: format!("path:{name}"), path: Some(name.into()) }).collect())
    }
    fn repository_status(&self, workspace: &WorkspaceId, id: &str) -> Result<lapis_git::GitSnapshot, GitError> {
        if self
            .failing_repository
            .lock()
            .unwrap()
            .as_deref()
            == Some(id)
        {
            return Err(GitError::new(
                lapis_git::GitErrorKind::Io,
                format!("{id} status failed"),
            ));
        }
        let revision = self.revisions.lock().unwrap().get(id).copied().unwrap_or(1);
        Ok(lapis_git::GitSnapshot { workspace_id: workspace.clone(), worktree_id: None, revision, branch: "main".into(), head: "abc".into(), files: vec![] })
    }
    fn repository_action(&self, _: &WorkspaceId, id: &str, _: lapis_git::GitActionRequest) -> Result<lapis_git::GitActionResult, GitError> {
        self.actions.lock().unwrap().push(id.into());
        Ok(lapis_git::GitActionResult { output: String::new(), diff: None, stashes: vec![] })
    }
    fn status(&self, _: &WorkspaceId, _: Option<&TaskId>) -> Result<lapis_git::GitSnapshot, GitError> { panic!("root is not a repository") }
    fn diff(&self, _: &WorkspaceId, _: Option<&TaskId>, _: &str) -> Result<lapis_git::GitDiffSnapshot, GitError> { panic!("unexpected diff") }
    fn worktrees(&self, _: &WorkspaceId) -> Result<Vec<lapis_git::WorktreeSnapshot>, GitError> { panic!("root is not a repository") }
    fn create_worktree(&self, _: &WorkspaceId) -> Result<lapis_git::WorktreeSnapshot, GitError> { panic!("unexpected worktree") }
    fn import_file(&self, _: &WorkspaceId, _: &TaskId, _: &str) -> Result<lapis_git::WorktreeSnapshot, GitError> { panic!("unexpected import") }
    fn discard_worktree(&self, _: &WorkspaceId, _: &TaskId) -> Result<lapis_git::WorktreeSnapshot, GitError> { panic!("unexpected discard") }
}

#[test]
fn git_session_groups_repositories_when_workspace_root_is_not_a_repository() {
    let fixture = Arc::new(GitFixture::default());
    let mut session = GitSession::new(fixture.clone());
    let workspace = WorkspaceId::new("project");
    session.refresh(&workspace).unwrap();
    assert_eq!(session.repository_statuses().len(), 2);
    assert!(session.status().is_none());
    session.refresh(&workspace).unwrap();
    assert_eq!(*fixture.discoveries.lock().unwrap(), 1);
    session.execute_repository_action("path:backend", lapis_git::GitActionRequest { action: lapis_git::GitAction::Stage { path: "file".into() }, expected_head: Some("abc".into()) }).unwrap();
    assert_eq!(&*fixture.actions.lock().unwrap(), &["path:backend"]);
    session.rescan_repositories().unwrap();
    assert_eq!(*fixture.discoveries.lock().unwrap(), 2);
}

#[test]
fn git_events_update_only_the_matching_repository_and_reject_old_revisions() {
    let fixture = Arc::new(GitFixture::default());
    let mut session = GitSession::new(fixture);
    let workspace = WorkspaceId::new("project");
    session.refresh(&workspace).unwrap();
    let mut status = session.repository_statuses()[0].1.clone();
    status.revision = 15;
    status.branch = "feature".into();
    let event = |status| lapis_git::GitUpdate::Changed(lapis_git::GitRepositoryStatus {
        repository_id: "path:frontend".into(),
        status,
    });
    assert!(session.apply_git_update(event(status.clone())));
    assert_eq!(session.repository_statuses()[0].1.branch, "feature");
    assert_eq!(session.repository_statuses()[1].1.revision, 1);
    status.revision = 14;
    status.branch = "stale".into();
    assert!(!session.apply_git_update(event(status)));
    assert_eq!(session.repository_statuses()[0].1.branch, "feature");
}


#[test]
fn git_session_keeps_other_repositories_fresh_when_one_status_fails() {
    let fixture = Arc::new(GitFixture::default());
    let mut session = GitSession::new(fixture.clone());
    let workspace = WorkspaceId::new("project");
    session.refresh(&workspace).unwrap();

    fixture
        .revisions
        .lock()
        .unwrap()
        .insert("path:frontend".into(), 2);
    *fixture.failing_repository.lock().unwrap() = Some("path:backend".into());

    assert!(session.refresh(&workspace).unwrap());
    let frontend = session
        .repository_statuses()
        .iter()
        .find(|(repository, _)| repository.id == "path:frontend")
        .unwrap();
    let backend = session
        .repository_statuses()
        .iter()
        .find(|(repository, _)| repository.id == "path:backend")
        .unwrap();
    assert_eq!(frontend.1.revision, 2);
    assert_eq!(backend.1.revision, 1);
    assert_eq!(session.repository_errors().len(), 1);
    assert_eq!(session.repository_errors()[0].0.id, "path:backend");

    session
        .execute_repository_action(
            "path:frontend",
            lapis_git::GitActionRequest {
                action: lapis_git::GitAction::Stage {
                    path: "file".into(),
                },
                expected_head: Some("abc".into()),
            },
        )
        .unwrap();
    assert_eq!(&*fixture.actions.lock().unwrap(), &["path:frontend"]);

    *fixture.failing_repository.lock().unwrap() = None;
    fixture
        .revisions
        .lock()
        .unwrap()
        .insert("path:backend".into(), 3);
    assert!(session.refresh(&workspace).unwrap());
    assert!(session.repository_errors().is_empty());
    assert_eq!(
        session
            .repository_statuses()
            .iter()
            .find(|(repository, _)| repository.id == "path:backend")
            .unwrap()
            .1
            .revision,
        3
    );
}


#[test]
fn git_status_event_recovers_repository_that_initially_failed() {
    let fixture = Arc::new(GitFixture::default());
    *fixture.failing_repository.lock().unwrap() = Some("path:backend".into());
    let mut session = GitSession::new(fixture.clone());
    let workspace = WorkspaceId::new("project");

    session.refresh(&workspace).unwrap();
    assert!(
        session
            .repository_statuses()
            .iter()
            .all(|(repository, _)| repository.id != "path:backend")
    );
    assert_eq!(session.repository_errors().len(), 1);

    let recovered = lapis_git::GitSnapshot {
        workspace_id: workspace,
        worktree_id: None,
        revision: 2,
        branch: "main".into(),
        head: "recovered".into(),
        files: vec![],
    };
    assert!(session.apply_git_update(lapis_git::GitUpdate::Changed(
        lapis_git::GitRepositoryStatus {
            repository_id: "path:backend".into(),
            status: recovered,
        },
    )));

    let backend = session
        .repository_statuses()
        .iter()
        .find(|(repository, _)| repository.id == "path:backend")
        .expect("recovered repository");
    assert_eq!(backend.1.revision, 2);
    assert!(session.repository_errors().is_empty());
}


#[test]
fn git_snapshot_update_is_applied_even_when_repository_reconciliation_fails() {
    let fixture = Arc::new(GitFixture::default());
    let mut session = GitSession::new(fixture.clone());
    let workspace = WorkspaceId::new("project");
    session.refresh(&workspace).unwrap();

    *fixture.fail_discovery.lock().unwrap() = true;
    let mut status = session
        .repository_statuses()
        .iter()
        .find(|(repository, _)| repository.id == "path:frontend")
        .unwrap()
        .1
        .clone();
    status.revision = 7;
    status.branch = "event-branch".into();

    assert!(session.reconcile_git_update(lapis_git::GitUpdate::Snapshot(vec![
        lapis_git::GitRepositoryStatus {
            repository_id: "path:frontend".into(),
            status,
        },
    ])));
    let frontend = session
        .repository_statuses()
        .iter()
        .find(|(repository, _)| repository.id == "path:frontend")
        .unwrap();
    assert_eq!(frontend.1.revision, 7);
    assert_eq!(frontend.1.branch, "event-branch");
    assert!(session.last_refresh_error().is_some());
}
