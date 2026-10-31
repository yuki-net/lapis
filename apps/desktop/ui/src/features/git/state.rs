use crate::client_adapter::{ClientWorker, ResponseGate};
use gpui::{Context, PromptButton, PromptLevel, Timer, Window};
use lapis_desktop_client::GitSession;
use lapis_editor_core::{TaskId, WorkspaceId};
use lapis_git::{GitAction, GitActionRequest, GitDiffSnapshot, GitSnapshot};
use std::time::Duration;

fn merge_repository_statuses(
    mut base: Vec<(lapis_git::GitRepositorySnapshot, GitSnapshot)>,
    updates: impl IntoIterator<Item = (lapis_git::GitRepositorySnapshot, GitSnapshot)>,
) -> Vec<(lapis_git::GitRepositorySnapshot, GitSnapshot)> {
    for (repository, incoming) in updates {
        if let Some((_, current)) = base
            .iter_mut()
            .find(|(existing, _)| existing.id == repository.id)
            && incoming.revision > current.revision
        {
            *current = incoming;
        }
    }
    base
}

#[cfg(test)]
mod event_order_tests {
    use super::*;
    use gpui::AppContext;
    use std::sync::{Arc, Mutex, mpsc};

    struct Backend {
        entered: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
        trees: Mutex<Vec<lapis_git::WorktreeSnapshot>>,
    }
    impl lapis_git::GitConnection for Backend {
        fn repositories(
            &self,
            _: &WorkspaceId,
        ) -> Result<Vec<lapis_git::GitRepositorySnapshot>, lapis_git::GitError> {
            Ok(vec![lapis_git::GitRepositorySnapshot {
                id: "root".into(),
                path: None,
            }])
        }
        fn repository_status(
            &self,
            workspace: &WorkspaceId,
            _: &str,
        ) -> Result<GitSnapshot, lapis_git::GitError> {
            self.status(workspace, None)
        }
        fn status(
            &self,
            workspace: &WorkspaceId,
            worktree: Option<&TaskId>,
        ) -> Result<GitSnapshot, lapis_git::GitError> {
            Ok(GitSnapshot {
                workspace_id: workspace.clone(),
                worktree_id: worktree.cloned(),
                revision: 1,
                branch: "main".into(),
                head: "abc".into(),
                files: vec![],
            })
        }
        fn worktrees(
            &self,
            _: &WorkspaceId,
        ) -> Result<Vec<lapis_git::WorktreeSnapshot>, lapis_git::GitError> {
            Ok(self.trees.lock().unwrap().clone())
        }
        fn execute(
            &self,
            _: &WorkspaceId,
            _: Option<&TaskId>,
            _: GitActionRequest,
        ) -> Result<lapis_git::GitActionResult, lapis_git::GitError> {
            self.entered.send(()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
            Err(lapis_git::GitError::new(
                lapis_git::GitErrorKind::Io,
                "mutation failed",
            ))
        }
        fn diff(
            &self,
            _: &WorkspaceId,
            _: Option<&TaskId>,
            _: &str,
        ) -> Result<GitDiffSnapshot, lapis_git::GitError> {
            unreachable!()
        }
        fn create_worktree(
            &self,
            _: &WorkspaceId,
        ) -> Result<lapis_git::WorktreeSnapshot, lapis_git::GitError> {
            unreachable!()
        }
        fn import_file(
            &self,
            _: &WorkspaceId,
            _: &TaskId,
            _: &str,
        ) -> Result<lapis_git::WorktreeSnapshot, lapis_git::GitError> {
            unreachable!()
        }
        fn discard_worktree(
            &self,
            _: &WorkspaceId,
            _: &TaskId,
        ) -> Result<lapis_git::WorktreeSnapshot, lapis_git::GitError> {
            unreachable!()
        }
    }

    #[test]
    fn repository_merge_preserves_membership_and_newest_revision() {
        let workspace = WorkspaceId::new("workspace");
        let repository = |id: &str| lapis_git::GitRepositorySnapshot {
            id: id.into(),
            path: Some(id.into()),
        };
        let status = |revision: u64| GitSnapshot {
            workspace_id: workspace.clone(),
            worktree_id: None,
            revision,
            branch: "main".into(),
            head: revision.to_string(),
            files: vec![],
        };

        let merged = merge_repository_statuses(
            vec![(repository("frontend"), status(2))],
            vec![
                (repository("frontend"), status(3)),
                (repository("backend"), status(9)),
            ],
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].0.id, "frontend");
        assert_eq!(merged[0].1.revision, 3);

        let authoritative_snapshot = merge_repository_statuses(
            vec![
                (repository("frontend"), status(2)),
                (repository("backend"), status(1)),
            ],
            vec![(repository("frontend"), status(4))],
        );
        assert_eq!(authoritative_snapshot.len(), 2);
        assert_eq!(authoritative_snapshot[0].1.revision, 4);
        assert_eq!(authoritative_snapshot[1].1.revision, 1);
    }

    #[gpui::test]
    fn status_event_does_not_hide_mutation_failure(cx: &mut gpui::TestAppContext) {
        let (entered, wait) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        let backend = Arc::new(Backend {
            entered,
            release: Mutex::new(resume),
            trees: Mutex::new(Vec::new()),
        });
        let workspace = WorkspaceId::new("workspace");
        let mut session = GitSession::new(backend);
        session.refresh(&workspace).unwrap();
        let feature = cx.update(|cx| cx.new(|cx| GitFeature::new(session, cx)));
        feature.update(cx, |git, cx| {
            git.workspace = Some(workspace.clone());
            git.submit(
                GitOperation::Action(
                    None,
                    GitActionRequest {
                        action: GitAction::Stage {
                            path: "file".into(),
                        },
                        expected_head: None,
                    },
                ),
                cx,
            );
        });
        wait.recv_timeout(Duration::from_secs(3)).unwrap();
        feature.update(cx, |git, cx| {
            git.submit(
                GitOperation::ApplyUpdate(lapis_git::GitUpdate::Changed(
                    lapis_git::GitRepositoryStatus {
                        repository_id: "root".into(),
                        status: GitSnapshot {
                            workspace_id: workspace.clone(),
                            worktree_id: None,
                            revision: 2,
                            branch: "main".into(),
                            head: "abc".into(),
                            files: vec![],
                        },
                    },
                )),
                cx,
            );
        });
        release.send(()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            cx.executor().advance_clock(Duration::from_millis(20));
            cx.run_until_parked();
            if cx.update(|cx| feature.read(cx).pending == 0) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        cx.update(|cx| {
            assert!(feature.read(cx).message.contains("mutation failed"));
            assert_eq!(
                feature.read(cx).snapshot.status.as_ref().unwrap().revision,
                2
            );
        });
    }

    #[test]
    fn worktree_refresh_reconciles_new_and_removed_instances() {
        let (entered, _) = mpsc::channel();
        let (_, resume) = mpsc::channel();
        let backend = Arc::new(Backend {
            entered,
            release: Mutex::new(resume),
            trees: Mutex::new(Vec::new()),
        });
        let workspace = WorkspaceId::new("workspace");
        let id = TaskId::new("worktree");
        let mut session = GitSession::new(backend.clone());
        session.refresh(&workspace).unwrap();
        backend
            .trees
            .lock()
            .unwrap()
            .push(lapis_git::WorktreeSnapshot {
                workspace_id: workspace,
                worktree_id: id.clone(),
                base_commit: "abc".into(),
                state: lapis_git::WorktreeState::Active,
                conflict: None,
            });
        session.refresh_worktrees().unwrap();
        assert_eq!(session.worktree_statuses()[0].0, id);
        backend.trees.lock().unwrap().clear();
        session.refresh_worktrees().unwrap();
        assert!(session.worktree_statuses().is_empty());
    }
}

#[derive(Default)]
pub(super) struct Snapshot {
    pub status: Option<GitSnapshot>,
    pub repositories: Vec<(lapis_git::GitRepositorySnapshot, GitSnapshot)>,
    pub worktree_statuses: Vec<(TaskId, GitSnapshot)>,
    pub selected_diff: Option<GitDiffSnapshot>,
    pub inspection: Option<lapis_desktop_client::GitDiffSelection>,
    pub output: String,
    pub stash_refs: Vec<(u32, String)>,
}
pub(super) enum GitOperation {
    Refresh,
    Import(TaskId, String),
    Discard(TaskId),
    Action(Option<TaskId>, GitActionRequest),
    RepositoryAction(String, GitActionRequest),
    RescanRepositories,
    Clear,
    CreateWorktree,
    ApplyUpdate(lapis_git::GitUpdate),
    RefreshWorktrees,
    RefreshRepositories,
}
#[derive(Clone, Copy)]
pub(crate) enum GitInputKind {
    Commit,
    Amend,
    CreateBranch,
    SwitchBranch,
    DeleteBranch,
    Merge,
    StashPush,
    StashApply,
    StashPop,
    StashDrop,
}
impl GitInputKind {
    pub fn title(self) -> &'static str {
        match self {
            Self::Commit => "Commit message",
            Self::Amend => "Amend message",
            Self::CreateBranch => "作成するBranch名",
            Self::SwitchBranch => "切り替えるBranch名",
            Self::DeleteBranch => "削除するBranch名",
            Self::Merge => "MergeするBranch名",
            Self::StashPush => "Stash message",
            Self::StashApply => "適用するStash番号（0から）",
            Self::StashPop => "PopするStash番号（0から）",
            Self::StashDrop => "削除するStash番号（0から）",
        }
    }
}
#[derive(Clone)]
pub(crate) struct GitInputRequest {
    kind: GitInputKind,
    workspace: WorkspaceId,
    worktree: Option<TaskId>,
    repository_id: Option<String>,
    head: String,
    stash_refs: Vec<(u32, String)>,
}
impl GitInputRequest {
    pub fn title(&self) -> &'static str {
        self.kind.title()
    }
}
#[derive(Clone)]
pub(crate) enum GitEvent {
    Input(GitInputRequest),
    DiscardWorktree {
        workspace: WorkspaceId,
        worktree: TaskId,
    },
    OpenWorktree {
        workspace: WorkspaceId,
        worktree: Option<TaskId>,
    },
}
pub(crate) struct GitFeature {
    worker: ClientWorker<GitSession>,
    backend: std::sync::Arc<dyn lapis_git::GitConnection>,
    subscription_generation: u64,
    worktree_refresh_generation: u64,
    repository_refresh_generation: u64,
    workspace: Option<WorkspaceId>,
    gate: ResponseGate,
    pending: usize,
    pub(super) snapshot: Snapshot,
    pub(super) message: String,
    message_from_refresh: bool,
    pub(super) target: Option<TaskId>,
    pub(super) context_label: String,
}
impl gpui::EventEmitter<GitEvent> for GitFeature {}
impl GitFeature {
    pub(super) fn open_task_worktree(&self, worktree: Option<TaskId>, cx: &mut Context<Self>) {
        if let Some(workspace) = self.workspace.clone() {
            cx.emit(GitEvent::OpenWorktree {
                workspace,
                worktree,
            });
        }
    }
    pub(crate) fn new(session: GitSession, _cx: &mut Context<Self>) -> Self {
        let backend = session.backend();
        Self {
            worker: ClientWorker::new(session),
            backend,
            subscription_generation: 0,
            worktree_refresh_generation: 0,
            repository_refresh_generation: 0,
            workspace: None,
            gate: ResponseGate::default(),
            pending: 0,
            snapshot: Snapshot::default(),
            message: String::new(),
            message_from_refresh: false,
            target: None,
            context_label: String::new(),
        }
    }
    pub(crate) fn set_workspace(&mut self, workspace: Option<WorkspaceId>, cx: &mut Context<Self>) {
        if self.workspace == workspace {
            return;
        }
        self.workspace = workspace;
        self.subscription_generation = self.subscription_generation.wrapping_add(1);
        self.target = None;
        self.gate.invalidate();
        self.snapshot = Snapshot::default();
        self.message.clear();
        self.message_from_refresh = false;
        self.submit(GitOperation::Refresh, cx);
        if let Some(workspace) = self.workspace.clone() {
            self.subscribe_updates(workspace, cx);
        }
        cx.notify();
    }
    fn subscribe_updates(&mut self, workspace: WorkspaceId, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        let generation = self.subscription_generation;
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            loop {
                let backend = backend.clone();
                let target = workspace.clone();
                let opened = executor
                    .spawn(async move { backend.subscribe(&target) })
                    .await;
                let mut subscription = match opened {
                    Ok(subscription) => subscription,
                    Err(_) => {
                        if !this
                            .update(cx, |git, _| {
                                git.workspace.as_ref() == Some(&workspace)
                                    && git.subscription_generation == generation
                            })
                            .unwrap_or(false)
                        {
                            break;
                        }
                        Timer::after(Duration::from_secs(2)).await;
                        continue;
                    }
                };
                loop {
                    let (next_subscription, update) = executor
                        .spawn(async move {
                            let update = subscription.recv_timeout(Duration::from_secs(1));
                            (subscription, update)
                        })
                        .await;
                    subscription = next_subscription;
                    let update = match update {
                        Ok(update) => update,
                        Err(_) => break,
                    };
                    let alive = this
                        .update(cx, |git, cx| {
                            if git.workspace.as_ref() != Some(&workspace)
                                || git.subscription_generation != generation
                            {
                                return false;
                            }
                            if let Some(update) = update {
                                if matches!(update, lapis_git::GitUpdate::RepositoriesChanged) {
                                    return git
                                        .submit_update(GitOperation::RefreshRepositories, cx);
                                }
                                if matches!(update, lapis_git::GitUpdate::WorktreesChanged) {
                                    return git.submit_update(GitOperation::RefreshWorktrees, cx);
                                }
                                return git.submit_update(GitOperation::ApplyUpdate(update), cx);
                            }
                            true
                        })
                        .unwrap_or(false);
                    if !alive {
                        if this
                            .update(cx, |git, _| {
                                git.workspace.as_ref() == Some(&workspace)
                                    && git.subscription_generation == generation
                            })
                            .unwrap_or(false)
                        {
                            break;
                        }
                        return;
                    }
                }
                Timer::after(Duration::from_secs(2)).await;
            }
        })
        .detach();
    }
    pub(super) fn submit(&mut self, operation: GitOperation, cx: &mut Context<Self>) {
        self.submit_update(operation, cx);
    }

    fn submit_update(&mut self, operation: GitOperation, cx: &mut Context<Self>) -> bool {
        let Some(workspace) = self.workspace.clone() else {
            return false;
        };
        if let GitOperation::Discard(worktree) = operation {
            cx.emit(GitEvent::DiscardWorktree {
                workspace,
                worktree,
            });
            return true;
        }
        let is_refresh = matches!(&operation, GitOperation::Refresh);
        let is_worktree_refresh = matches!(
            &operation,
            GitOperation::RefreshWorktrees
                | GitOperation::ApplyUpdate(lapis_git::GitUpdate::Snapshot(_))
        );
        let is_repository_refresh = matches!(
            &operation,
            GitOperation::RefreshRepositories
                | GitOperation::RescanRepositories
                | GitOperation::ApplyUpdate(lapis_git::GitUpdate::Snapshot(_))
        );
        let is_event = matches!(
            &operation,
            GitOperation::ApplyUpdate(_)
                | GitOperation::RefreshWorktrees
                | GitOperation::RefreshRepositories
        );
        let token = (!is_event).then(|| self.gate.issue());
        let generation = self.subscription_generation;
        if is_worktree_refresh {
            self.worktree_refresh_generation = self.worktree_refresh_generation.wrapping_add(1);
        }
        let worktree_generation = self.worktree_refresh_generation;
        if is_repository_refresh {
            self.repository_refresh_generation = self.repository_refresh_generation.wrapping_add(1);
        }
        let repository_generation = self.repository_refresh_generation;
        self.pending += 1;
        let admitted = self.worker.submit(
            cx,
            move |session| {
                if !matches!(
                    &operation,
                    GitOperation::ApplyUpdate(_)
                        | GitOperation::RefreshWorktrees
                        | GitOperation::RefreshRepositories
                        | GitOperation::RescanRepositories
                ) {
                    session
                        .refresh(&workspace)
                        .map_err(|error| error.to_string())?;
                }
                let result = match operation {
                    GitOperation::Refresh => Ok(()),
                    GitOperation::RefreshRepositories => session.rescan_repositories(),
                    GitOperation::RefreshWorktrees => session.refresh_worktrees(),
                    GitOperation::ApplyUpdate(update) => {
                        let is_snapshot = matches!(&update, lapis_git::GitUpdate::Snapshot(_));
                        let rescanned = if is_snapshot {
                            session
                                .rescan_repositories()
                                .map_err(|error| error.to_string())
                        } else {
                            Ok(())
                        };
                        let refreshed = if is_snapshot && session.status().is_some() {
                            session
                                .refresh_worktrees()
                                .map_err(|error| error.to_string())
                        } else {
                            Ok(())
                        };
                        session.apply_git_update(update);
                        rescanned.and(refreshed)
                    }
                    GitOperation::Import(task, path) => session.import_file(&task, &path),
                    GitOperation::Discard(_) => unreachable!("Task状態の所有者で検証する"),
                    GitOperation::Action(worktree, request) => {
                        session.execute_action(worktree.as_ref(), request)
                    }
                    GitOperation::RepositoryAction(repository_id, request) => {
                        session.execute_repository_action(&repository_id, request)
                    }
                    GitOperation::RescanRepositories => session.rescan_repositories(),
                    GitOperation::Clear => {
                        session.clear_inspection();
                        Ok(())
                    }
                    GitOperation::CreateWorktree => session.create_worktree(),
                };
                result.map_err(|error| error.to_string())?;
                Ok(Snapshot {
                    status: session.status().cloned(),
                    repositories: session.repository_statuses().to_vec(),
                    worktree_statuses: session.worktree_statuses().to_vec(),
                    selected_diff: session.selected_diff().cloned(),
                    inspection: session.inspection().cloned(),
                    output: session.action_output().to_owned(),
                    stash_refs: session.stash_refs().to_vec(),
                })
            },
            move |git, result, cx| {
                git.pending = git.pending.saturating_sub(1);
                if git.subscription_generation != generation
                    || token.is_some_and(|token| !git.gate.accept(token))
                {
                    return;
                }
                match result {
                    Ok(mut snapshot) => {
                        if is_event {
                            if is_repository_refresh
                                && git.repository_refresh_generation == repository_generation
                            {
                                git.snapshot.repositories = merge_repository_statuses(
                                    snapshot.repositories,
                                    git.snapshot.repositories.clone(),
                                );
                            } else {
                                git.snapshot.repositories = merge_repository_statuses(
                                    git.snapshot.repositories.clone(),
                                    snapshot.repositories,
                                );
                            }
                            if snapshot.status.as_ref().is_some_and(|incoming| {
                                git.snapshot
                                    .status
                                    .as_ref()
                                    .is_none_or(|current| incoming.revision > current.revision)
                            }) {
                                git.snapshot.status = snapshot.status;
                            }
                            if is_worktree_refresh
                                && git.worktree_refresh_generation == worktree_generation
                            {
                                let mut incoming = snapshot.worktree_statuses;
                                for (id, status) in &mut incoming {
                                    if let Some((_, current)) = git
                                        .snapshot
                                        .worktree_statuses
                                        .iter()
                                        .find(|(existing, _)| existing == id)
                                        && current.revision > status.revision
                                    {
                                        *status = current.clone();
                                    }
                                }
                                git.snapshot.worktree_statuses = incoming;
                            } else {
                                for (id, incoming) in snapshot.worktree_statuses {
                                    if let Some((_, current)) = git
                                        .snapshot
                                        .worktree_statuses
                                        .iter_mut()
                                        .find(|(existing, _)| *existing == id)
                                        && incoming.revision > current.revision
                                    {
                                        *current = incoming;
                                    }
                                }
                            }
                        } else {
                            if let (Some(current), Some(incoming)) =
                                (&git.snapshot.status, &snapshot.status)
                                && incoming.revision < current.revision
                            {
                                snapshot.status = Some(current.clone());
                            }
                            if git.worktree_refresh_generation != worktree_generation {
                                snapshot.worktree_statuses = git.snapshot.worktree_statuses.clone();
                            } else {
                                for (id, incoming) in &mut snapshot.worktree_statuses {
                                    if let Some((_, current)) = git
                                        .snapshot
                                        .worktree_statuses
                                        .iter()
                                        .find(|(existing, _)| existing == id)
                                        && current.revision > incoming.revision
                                    {
                                        *incoming = current.clone();
                                    }
                                }
                            }
                            snapshot.repositories =
                                if git.repository_refresh_generation != repository_generation {
                                    merge_repository_statuses(
                                        git.snapshot.repositories.clone(),
                                        snapshot.repositories,
                                    )
                                } else {
                                    merge_repository_statuses(
                                        snapshot.repositories,
                                        git.snapshot.repositories.clone(),
                                    )
                                };
                            git.snapshot = snapshot;
                        }
                        if git.target.as_ref().is_some_and(|target| {
                            !git.snapshot
                                .worktree_statuses
                                .iter()
                                .any(|(id, _)| id == target)
                        }) {
                            git.target = None;
                        }
                        if !is_event && (!is_refresh || git.message_from_refresh) {
                            git.message.clear();
                            git.message_from_refresh = false;
                        }
                    }
                    Err(error) => {
                        if !is_event {
                            git.message = error;
                            git.message_from_refresh = is_refresh;
                        }
                    }
                }
                cx.notify();
            },
        );
        if let Err(error) = admitted {
            self.pending = self.pending.saturating_sub(1);
            if !is_event {
                self.message = error;
                self.message_from_refresh = is_refresh;
            }
            cx.notify();
            return false;
        }
        true
    }

    pub(crate) fn set_context_label(&mut self, label: String) {
        self.context_label = label;
    }
    pub(super) fn selected_status(&self) -> Option<&GitSnapshot> {
        match self.target.as_ref() {
            None => self.snapshot.status.as_ref(),
            Some(id) => self
                .snapshot
                .worktree_statuses
                .iter()
                .find(|(target, _)| target == id)
                .map(|(_, status)| status),
        }
    }
    pub(super) fn select_target(&mut self, target: Option<TaskId>, cx: &mut Context<Self>) {
        self.target = target;
        self.submit(GitOperation::Clear, cx);
    }
    pub(super) fn prompt_input(&mut self, kind: GitInputKind, cx: &mut Context<Self>) {
        if let (Some(workspace), Some(status)) = (&self.workspace, self.selected_status()) {
            cx.emit(GitEvent::Input(GitInputRequest {
                kind,
                workspace: workspace.clone(),
                worktree: self.target.clone(),
                repository_id: None,
                head: status.head.clone(),
                stash_refs: self.snapshot.stash_refs.clone(),
            }));
        }
    }
    pub(crate) fn complete_input(
        &mut self,
        request: GitInputRequest,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current_head = if let Some(id) = &request.repository_id {
            self.repository_status(id).map(|status| &status.head)
        } else {
            self.selected_status().map(|status| &status.head)
        };
        if self.workspace.as_ref() != Some(&request.workspace)
            || (request.repository_id.is_none() && self.target != request.worktree)
            || current_head != Some(&request.head)
        {
            self.message = "操作対象が変わりました。Git操作を選び直してください".into();
            cx.notify();
            return;
        }
        let index = || {
            let index = text
                .trim()
                .parse::<u32>()
                .map_err(|_| "Stash番号は0以上の整数で指定してください")?;
            let oid = request
                .stash_refs
                .iter()
                .find(|(number, _)| *number == index)
                .map(|(_, oid)| oid.clone())
                .ok_or("先にStash一覧を更新し、表示された番号を選んでください")?;
            Ok::<_, &str>((index, oid))
        };
        let action = match request.kind {
            GitInputKind::Commit => Ok(GitAction::Commit {
                message: text.clone(),
            }),
            GitInputKind::Amend => Ok(GitAction::Amend {
                message: text.clone(),
            }),
            GitInputKind::CreateBranch => Ok(GitAction::CreateBranch { name: text.clone() }),
            GitInputKind::SwitchBranch => Ok(GitAction::SwitchBranch { name: text.clone() }),
            GitInputKind::DeleteBranch => Ok(GitAction::DeleteBranch { name: text.clone() }),
            GitInputKind::Merge => Ok(GitAction::Merge {
                branch: text.clone(),
            }),
            GitInputKind::StashPush => Ok(GitAction::StashPush {
                message: text.clone(),
            }),
            GitInputKind::StashApply => {
                index().map(|(index, expected_oid)| GitAction::StashApply {
                    index,
                    expected_oid,
                })
            }
            GitInputKind::StashPop => index().map(|(index, expected_oid)| GitAction::StashPop {
                index,
                expected_oid,
            }),
            GitInputKind::StashDrop => index().map(|(index, expected_oid)| GitAction::StashDrop {
                index,
                expected_oid,
            }),
        };
        match action {
            Ok(action) => {
                if let Some(repository_id) = request.repository_id {
                    self.execute_repository_action(repository_id, action, window, cx);
                } else {
                    self.execute_action(action, window, cx);
                }
            }
            Err(error) => {
                self.message = error.into();
                cx.notify();
            }
        }
    }
    pub(super) fn repository_status(&self, id: &str) -> Option<&GitSnapshot> {
        self.snapshot
            .repositories
            .iter()
            .find(|(repository, _)| repository.id == id)
            .map(|(_, status)| status)
    }
    pub(super) fn prompt_repository_commit(
        &mut self,
        repository_id: String,
        amend: bool,
        cx: &mut Context<Self>,
    ) {
        if let (Some(workspace), Some(status)) =
            (&self.workspace, self.repository_status(&repository_id))
        {
            cx.emit(GitEvent::Input(GitInputRequest {
                kind: if amend {
                    GitInputKind::Amend
                } else {
                    GitInputKind::Commit
                },
                workspace: workspace.clone(),
                worktree: None,
                repository_id: Some(repository_id),
                head: status.head.clone(),
                stash_refs: Vec::new(),
            }));
        }
    }
    pub(super) fn execute_repository_action(
        &mut self,
        repository_id: String,
        action: GitAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(workspace), Some(status)) = (
            self.workspace.clone(),
            self.repository_status(&repository_id),
        ) else {
            return;
        };
        let head = status.head.clone();
        let request = GitActionRequest {
            expected_head: action.is_mutation().then_some(head.clone()),
            action: action.clone(),
        };
        if !action.needs_confirmation() {
            self.submit(GitOperation::RepositoryAction(repository_id, request), cx);
            return;
        }
        let operation = match &action {
            GitAction::Restore { path, .. } => format!("Restore {path}"),
            GitAction::Amend { .. } => "Amend".to_owned(),
            _ => format!("{:?}", action),
        };
        let detail = format!("Repository: {}\n{}", repository_id, operation);
        let answer = window.prompt(
            PromptLevel::Warning,
            "Git operation?",
            Some(&detail),
            &[PromptButton::new("Run"), PromptButton::cancel("Cancel")],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let _ = this.update(cx, |feature, cx| {
                if feature.workspace.as_ref() == Some(&workspace)
                    && feature
                        .repository_status(&repository_id)
                        .map(|status| &status.head)
                        == Some(&head)
                {
                    feature.submit(GitOperation::RepositoryAction(repository_id, request), cx);
                } else {
                    feature.message = "Repository changed; choose the operation again".into();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn execute_action(
        &mut self,
        action: GitAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(workspace), Some(status)) = (self.workspace.clone(), self.selected_status())
        else {
            return;
        };
        let head = status.head.clone();
        let target = self.target.clone();
        let request = GitActionRequest {
            expected_head: action.is_mutation().then_some(head.clone()),
            action: action.clone(),
        };
        if !action.needs_confirmation() {
            self.submit(GitOperation::Action(target, request), cx);
            return;
        }
        let detail = format!(
            "対象: {}\nWorktree: {}\n操作: {:?}\n作業ファイルやGitの状態が変わります。",
            self.context_label,
            target.as_ref().map_or("登録した環境", |id| id.as_str()),
            action
        );
        let answer = window.prompt(
            PromptLevel::Warning,
            "Git操作を実行しますか？",
            Some(&detail),
            &[PromptButton::new("実行"), PromptButton::cancel("Cancel")],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let _ = this.update(cx, |feature, cx| {
                if feature.workspace.as_ref() == Some(&workspace)
                    && feature.target == target
                    && feature.selected_status().map(|status| &status.head) == Some(&head)
                {
                    feature.submit(GitOperation::Action(target, request), cx);
                } else {
                    feature.message =
                        "確認中に操作対象が変わりました。操作を選び直してください".into();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn confirm_operation(
        &mut self,
        operation: GitOperation,
        label: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let workspace = self.workspace.clone();
        let target = self.target.clone();
        let head = self
            .snapshot
            .status
            .as_ref()
            .map(|status| status.head.clone());
        let detail = format!(
            "{}\n対象: {}\n未Commit変更やBranchに保存していないCommitがあるWorktreeは削除できません。",
            label, self.context_label
        );
        let answer = window.prompt(
            PromptLevel::Warning,
            "Git操作を実行しますか？",
            Some(&detail),
            &[PromptButton::new("実行"), PromptButton::cancel("Cancel")],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let _ = this.update(cx, |feature, cx| {
                if feature.workspace == workspace
                    && feature.target == target
                    && feature
                        .snapshot
                        .status
                        .as_ref()
                        .map(|status| status.head.clone())
                        == head
                {
                    feature.submit(operation, cx);
                } else {
                    feature.message =
                        "確認中に操作対象が変わりました。操作を選び直してください".into();
                    cx.notify();
                }
            });
        })
        .detach();
    }
}
