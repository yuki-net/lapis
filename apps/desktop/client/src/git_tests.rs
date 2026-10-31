#[derive(Default)]
struct GitFixture {
    discoveries: Mutex<usize>,
    actions: Mutex<Vec<String>>,
}

impl lapis_git::GitConnection for GitFixture {
    fn repositories(&self, _: &WorkspaceId) -> Result<Vec<lapis_git::GitRepositorySnapshot>, GitError> {
        *self.discoveries.lock().unwrap() += 1;
        Ok(["frontend", "backend"].into_iter().map(|name| lapis_git::GitRepositorySnapshot { id: format!("path:{name}"), path: Some(name.into()) }).collect())
    }
    fn repository_status(&self, workspace: &WorkspaceId, _: &str) -> Result<lapis_git::GitSnapshot, GitError> {
        Ok(lapis_git::GitSnapshot { workspace_id: workspace.clone(), worktree_id: None, revision: 1, branch: "main".into(), head: "abc".into(), files: vec![] })
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
