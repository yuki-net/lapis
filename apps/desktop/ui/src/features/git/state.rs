use crate::client_adapter::{ClientWorker, ResponseGate};
use gpui::{Context, Timer};
use lapis_desktop_client::GitSession;
use lapis_editor_core::{TaskId, WorkspaceId};
use lapis_git::{GitDiffSnapshot, GitSnapshot};
use std::time::Duration;

#[derive(Default)]
pub(super) struct Snapshot {
    pub status: Option<GitSnapshot>,
    pub worktree_statuses: Vec<(TaskId, GitSnapshot)>,
    pub selected_diff: Option<GitDiffSnapshot>,
}
pub(super) enum GitOperation {
    Refresh,
    Diff(String),
    WorktreeDiff(TaskId, String),
    Import(TaskId, String),
    Discard(TaskId),
}
pub(crate) struct GitFeature {
    worker: ClientWorker<GitSession>,
    workspace: Option<WorkspaceId>,
    gate: ResponseGate,
    pending: usize,
    pub(super) snapshot: Snapshot,
    pub(super) message: String,
    message_from_refresh: bool,
}
impl GitFeature {
    pub(crate) fn new(session: GitSession, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_secs(1)).await;
                if this
                    .update(cx, |git, cx| {
                        if git.pending == 0 {
                            git.submit(GitOperation::Refresh, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Self {
            worker: ClientWorker::new(session),
            workspace: None,
            gate: ResponseGate::default(),
            pending: 0,
            snapshot: Snapshot::default(),
            message: String::new(),
            message_from_refresh: false,
        }
    }
    pub(crate) fn set_workspace(&mut self, workspace: Option<WorkspaceId>, cx: &mut Context<Self>) {
        if self.workspace == workspace {
            return;
        }
        self.workspace = workspace;
        self.gate.invalidate();
        self.snapshot = Snapshot::default();
        self.message.clear();
        self.message_from_refresh = false;
        self.submit(GitOperation::Refresh, cx);
        cx.notify();
    }
    pub(super) fn submit(&mut self, operation: GitOperation, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        let is_refresh = matches!(&operation, GitOperation::Refresh);
        let token = self.gate.issue();
        self.pending += 1;
        let admitted = self.worker.submit(
            cx,
            move |session| {
                session
                    .refresh(&workspace)
                    .map_err(|error| error.to_string())?;
                let result = match operation {
                    GitOperation::Refresh => Ok(()),
                    GitOperation::Diff(path) => session.select_diff(&path),
                    GitOperation::WorktreeDiff(task, path) => {
                        session.select_worktree_diff(&task, &path)
                    }
                    GitOperation::Import(task, path) => session.import_file(&task, &path),
                    GitOperation::Discard(task) => session.discard_worktree(&task),
                };
                result.map_err(|error| error.to_string())?;
                Ok(Snapshot {
                    status: session.status().cloned(),
                    worktree_statuses: session.worktree_statuses().to_vec(),
                    selected_diff: session.selected_diff().cloned(),
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
        }
    }
}
