use crate::client_adapter::{ClientWorker, ResponseGate};
use gpui::{Context, PromptButton, PromptLevel, Timer, Window};
use lapis_desktop_client::GitSession;
use lapis_editor_core::{TaskId, WorkspaceId};
use lapis_git::{GitAction, GitActionRequest, GitDiffSnapshot, GitSnapshot};
use std::time::Duration;

#[derive(Default)]
pub(super) struct Snapshot {
    pub status: Option<GitSnapshot>,
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
    Clear,
    CreateWorktree,
    ApplyUpdate(lapis_git::GitUpdate),
}
#[derive(Clone, Copy)]
pub(crate) enum GitInputKind {
    Commit,
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
        let token = self.gate.issue();
        self.pending += 1;
        let admitted = self.worker.submit(
            cx,
            move |session| {
                if !matches!(&operation, GitOperation::ApplyUpdate(_)) {
                    session
                        .refresh(&workspace)
                        .map_err(|error| error.to_string())?;
                }
                let result = match operation {
                    GitOperation::Refresh => Ok(()),
                    GitOperation::ApplyUpdate(update) => {
                        session.apply_git_update(update);
                        Ok(())
                    }
                    GitOperation::Import(task, path) => session.import_file(&task, &path),
                    GitOperation::Discard(_) => unreachable!("Task状態の所有者で検証する"),
                    GitOperation::Action(worktree, request) => {
                        session.execute_action(worktree.as_ref(), request)
                    }
                    GitOperation::Clear => {
                        session.clear_inspection();
                        Ok(())
                    }
                    GitOperation::CreateWorktree => session.create_worktree(),
                };
                result.map_err(|error| error.to_string())?;
                Ok(Snapshot {
                    status: session.status().cloned(),
                    worktree_statuses: session.worktree_statuses().to_vec(),
                    selected_diff: session.selected_diff().cloned(),
                    inspection: session.inspection().cloned(),
                    output: session.action_output().to_owned(),
                    stash_refs: session.stash_refs().to_vec(),
                })
            },
            move |git, result, cx| {
                git.pending = git.pending.saturating_sub(1);
                if !git.gate.accept(token) {
                    return;
                }
                match result {
                    Ok(snapshot) => {
                        git.snapshot = snapshot;
                        if git.target.as_ref().is_some_and(|target| {
                            !git.snapshot
                                .worktree_statuses
                                .iter()
                                .any(|(id, _)| id == target)
                        }) {
                            git.target = None;
                        }
                        if !is_refresh || git.message_from_refresh {
                            git.message.clear();
                            git.message_from_refresh = false;
                        }
                    }
                    Err(error) => {
                        git.message = error;
                        git.message_from_refresh = is_refresh;
                    }
                }
                cx.notify();
            },
        );
        if let Err(error) = admitted {
            self.pending = self.pending.saturating_sub(1);
            self.message = error;
            self.message_from_refresh = is_refresh;
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
        if self.workspace.as_ref() != Some(&request.workspace)
            || self.target != request.worktree
            || self.selected_status().map(|status| &status.head) != Some(&request.head)
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
            Ok(action) => self.execute_action(action, window, cx),
            Err(error) => {
                self.message = error.into();
                cx.notify();
            }
        }
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
