use crate::timer::Timer;
use crate::{
    client_adapter::{ClientWorker, ResponseGate},
    components::ScrollState,
    theme,
};
use desktop_client::TaskSession;
use editor_core::{ExecutionId, WorkspaceId};
use gpui::Context;
use std::{sync::Arc, time::Duration};
use task::{ExecutionStatus, TaskControl, TaskMode, TaskSnapshot};
pub(crate) struct TasksFeature {
    worker: ClientWorker<TaskSession>,
    backend: Arc<dyn task::TaskConnection>,
    subscription_generation: u64,
    pub(super) records: Vec<TaskSnapshot>,
    pub(crate) selected_execution: Option<ExecutionId>,
    workspace: Option<WorkspaceId>,
    conversation: Option<editor_core::ConversationId>,
    pub(super) selected_text: Option<String>,
    pending: usize,
    sync_pending: bool,
    gate: ResponseGate,
    pub(super) message: String,
    message_from_refresh: bool,
    pub(super) scroll_states: TaskScrollStates,
}
pub(super) struct TaskScrollStates {
    pub(super) task_list: ScrollState,
    pub(super) task_events: ScrollState,
}
impl TasksFeature {
    pub(crate) fn new(session: TaskSession, _cx: &mut Context<Self>) -> Self {
        let backend = session.backend();
        Self {
            worker: ClientWorker::new(session),
            backend,
            subscription_generation: 0,
            records: Vec::new(),
            selected_execution: None,
            workspace: None,
            conversation: None,
            selected_text: None,
            pending: 0,
            sync_pending: false,
            gate: ResponseGate::default(),
            message: String::new(),
            message_from_refresh: false,
            scroll_states: TaskScrollStates {
                task_list: ScrollState::new(),
                task_events: ScrollState::new(),
            },
        }
    }
    pub(crate) fn set_conversation(&mut self, conversation: editor_core::ConversationId) {
        if self.conversation.as_ref() != Some(&conversation) {
            self.gate.invalidate();
            self.selected_execution = None;
        }
        self.conversation = Some(conversation);
    }
    pub(crate) fn set_selected_text(&mut self, text: Option<String>) {
        self.selected_text = text;
    }

    /// TaskはAgent Sessionを持たなくても成立する。Assistant表示だけを
    /// 選択中Taskへ絞り、別Taskの実行を勝手に追加・複製しない。
    pub(super) fn visible_records(&self) -> Vec<&TaskSnapshot> {
        records_for_conversation(&self.records, self.conversation.as_ref())
    }

    pub(crate) fn set_workspace(&mut self, workspace: Option<WorkspaceId>, cx: &mut Context<Self>) {
        if self.workspace == workspace {
            return;
        }
        self.workspace = workspace.clone();
        self.subscription_generation = self.subscription_generation.wrapping_add(1);
        self.gate.invalidate();
        self.records.clear();
        self.selected_execution = None;
        self.message.clear();
        self.message_from_refresh = false;
        self.sync_pending = false;
        self.refresh(cx);
        if let Some(workspace) = workspace {
            self.subscribe_updates(workspace, cx);
        }
        cx.notify();
    }

    /// Host の Task 変更通知で取得する。欠落時は再同期して購読し直す。
    fn subscribe_updates(&mut self, workspace: WorkspaceId, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        let generation = self.subscription_generation;
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            loop {
                let target = workspace.clone();
                let backend = backend.clone();
                let opened = executor
                    .spawn(async move { backend.subscribe(&target) })
                    .await;
                let Some(mut subscription) = opened else {
                    return;
                };
                // 購読開始前の変更を回収する。
                let alive = this
                    .update(cx, |tasks, cx| {
                        if tasks.subscription_generation != generation
                            || tasks.workspace.as_ref() != Some(&workspace)
                        {
                            return false;
                        }
                        if tasks.pending == 0 {
                            tasks.refresh(cx);
                        } else {
                            tasks.sync_pending = true;
                        }
                        true
                    })
                    .unwrap_or(false);
                if !alive {
                    return;
                }
                loop {
                    let (next, outcome) = executor
                        .spawn(async move {
                            let outcome = subscription.recv_timeout(Duration::from_secs(1));
                            (subscription, outcome)
                        })
                        .await;
                    subscription = next;
                    match outcome {
                        Ok(true) => {
                            let alive = this
                                .update(cx, |tasks, cx| {
                                    if tasks.subscription_generation != generation
                                        || tasks.workspace.as_ref() != Some(&workspace)
                                    {
                                        return false;
                                    }
                                    if tasks.pending == 0 {
                                        tasks.refresh(cx);
                                    } else {
                                        tasks.sync_pending = true;
                                    }
                                    true
                                })
                                .unwrap_or(false);
                            if !alive {
                                return;
                            }
                        }
                        Ok(false) => {}
                        // 通知の欠落・切断。再同期してから購読し直す。
                        Err(_) => break,
                    }
                }
                let alive = this
                    .update(cx, |tasks, cx| {
                        if tasks.subscription_generation != generation
                            || tasks.workspace.as_ref() != Some(&workspace)
                        {
                            return false;
                        }
                        if tasks.pending == 0 {
                            tasks.refresh(cx);
                        } else {
                            tasks.sync_pending = true;
                        }
                        true
                    })
                    .unwrap_or(false);
                if !alive {
                    return;
                }
                Timer::after(Duration::from_secs(2)).await;
            }
        })
        .detach();
    }
    pub(super) fn start_codex_task(
        &mut self,
        mode: TaskMode,
        isolated: bool,
        cx: &mut Context<Self>,
    ) {
        let prompt = self
            .selected_text
            .clone()
            .or_else(|| cx.read_from_clipboard().and_then(|item| item.text()))
            .unwrap_or_else(|| "このWorkspaceを読み取り専用で調査してください".into());
        self.start(prompt, mode, isolated, cx);
    }
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.operation_with(true, |_, _| Ok(None), cx);
    }
    fn operation(
        &mut self,
        operation: impl FnOnce(&mut TaskSession, WorkspaceId) -> Result<Option<ExecutionId>, String>
        + Send
        + 'static,
        cx: &mut Context<Self>,
    ) {
        self.operation_with(false, operation, cx);
    }
    fn operation_with(
        &mut self,
        is_refresh: bool,
        operation: impl FnOnce(&mut TaskSession, WorkspaceId) -> Result<Option<ExecutionId>, String>
        + Send
        + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        let conversation = self.conversation.clone();
        let token = self.gate.issue();
        self.pending += 1;
        let admitted = self.worker.submit(
            cx,
            move |session| {
                if let Some(conversation) = conversation {
                    session.set_conversation(conversation);
                }
                let selected = operation(session, workspace.clone())?;
                session
                    .refresh(&workspace)
                    .map_err(|error| error.to_string())?;
                Ok((session.records().to_vec(), selected))
            },
            move |tasks, result, cx| {
                tasks.pending = tasks.pending.saturating_sub(1);
                if tasks.gate.accept(token) {
                    match result {
                        Ok((records, selected)) => {
                            tasks.records = records;
                            tasks.selected_execution = select_visible_execution(
                                &tasks.visible_records(),
                                selected,
                                tasks.selected_execution.clone(),
                            );
                            if !is_refresh || tasks.message_from_refresh {
                                tasks.message.clear();
                                tasks.message_from_refresh = false;
                            }
                        }
                        Err(error) => {
                            tasks.message = error;
                            tasks.message_from_refresh = is_refresh;
                        }
                    }
                    cx.notify();
                }
                if tasks.pending == 0 && tasks.sync_pending {
                    // 通知を処理中だった分は、結果の適用後に取り直す。
                    tasks.sync_pending = false;
                    tasks.refresh(cx);
                }
            },
        );
        if let Err(error) = admitted {
            self.pending = self.pending.saturating_sub(1);
            self.message = error;
            self.message_from_refresh = is_refresh;
            cx.notify();
        }
    }
    pub(crate) fn start(
        &mut self,
        prompt: String,
        mode: TaskMode,
        isolated: bool,
        cx: &mut Context<Self>,
    ) {
        self.operation(
            move |session, workspace| {
                let result = if isolated {
                    session.start_codex_in_worktree(workspace, prompt, mode)
                } else {
                    session.start_codex_with_mode(workspace, prompt, mode)
                };
                result.map(Some).map_err(|error| error.to_string())
            },
            cx,
        );
    }
    pub(super) fn select_execution(&mut self, id: ExecutionId, cx: &mut Context<Self>) {
        self.selected_execution = select_visible_execution(
            &self.visible_records(),
            Some(id),
            self.selected_execution.clone(),
        );
        cx.notify();
    }
    pub(super) fn control_selected_task(&mut self, control: TaskControl, cx: &mut Context<Self>) {
        let Some(id) = self.selected_execution.clone() else {
            return;
        };
        if !self
            .visible_records()
            .iter()
            .any(|record| record.execution.id == id)
        {
            return;
        }
        self.operation(
            move |session, _| {
                session
                    .control(&id, control)
                    .map(|_| None)
                    .map_err(|error| error.to_string())
            },
            cx,
        );
    }
    pub(super) fn reply_to_selected_task(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.control_selected_task(TaskControl::Reply { text }, cx);
        }
    }
}
pub(super) fn truncate_chars(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}
pub(super) fn task_status_color(status: ExecutionStatus, cx: &gpui::App) -> gpui::Rgba {
    match status {
        ExecutionStatus::Succeeded => theme::colors(cx).positive_text,
        ExecutionStatus::Failed | ExecutionStatus::Cancelled => theme::colors(cx).danger_text,
        ExecutionStatus::WaitingForInput | ExecutionStatus::WaitingForApproval => {
            theme::colors(cx).warning_text
        }
        _ => theme::colors(cx).info_text,
    }
}

fn records_for_conversation<'a>(
    records: &'a [TaskSnapshot],
    conversation: Option<&editor_core::ConversationId>,
) -> Vec<&'a TaskSnapshot> {
    records
        .iter()
        .filter(|record| conversation.is_none_or(|id| &record.task.conversation_id == id))
        .collect()
}

fn select_visible_execution(
    records: &[&TaskSnapshot],
    requested: Option<ExecutionId>,
    previous: Option<ExecutionId>,
) -> Option<ExecutionId> {
    let visible = |id: &ExecutionId| records.iter().any(|record| &record.execution.id == id);
    requested
        .filter(visible)
        .or_else(|| previous.filter(visible))
        .or_else(|| records.first().map(|record| record.execution.id.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::{ConversationId, TaskId};
    use task::{ExecutionSnapshot, Task};

    fn session(conversation: &str, id: &str) -> TaskSnapshot {
        TaskSnapshot {
            task: Task {
                id: TaskId::new(id),
                conversation_id: ConversationId::new(conversation),
                title: id.into(),
                prompt: String::new(),
                created_at_ms: 0,
            },
            execution: ExecutionSnapshot {
                id: ExecutionId::new(id),
                task_id: TaskId::new(id),
                workspace_id: WorkspaceId::new("workspace"),
                worktree_id: None,
                runner: "codex".into(),
                mode: TaskMode::Default,
                status: ExecutionStatus::Running,
                started_at_ms: None,
                completed_at_ms: None,
                external_thread_id: None,
                failure: None,
            },
            events: Vec::new(),
        }
    }

    #[test]
    fn task_without_agent_sessions_is_valid() {
        let records = vec![
            session("task-a", "execution-a"),
            session("task-b", "execution-b"),
        ];
        assert!(
            records_for_conversation(&records, Some(&ConversationId::new("task-without-session")))
                .is_empty()
        );
        let visible = records_for_conversation(&records, Some(&ConversationId::new("task-a")));
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].execution.id.as_str(), "execution-a");
        assert_eq!(records_for_conversation(&records, None).len(), 2);
    }

    #[test]
    fn execution_selection_never_crosses_the_visible_task() {
        let records = vec![
            session("task-b", "foreign"),
            session("task-a", "first"),
            session("task-a", "second"),
        ];
        let visible = records_for_conversation(&records, Some(&ConversationId::new("task-a")));
        assert_eq!(
            select_visible_execution(&visible, Some(ExecutionId::new("foreign")), None),
            Some(ExecutionId::new("first"))
        );
        assert_eq!(
            select_visible_execution(&visible, None, Some(ExecutionId::new("foreign"))),
            Some(ExecutionId::new("first"))
        );
        assert_eq!(
            select_visible_execution(&visible, None, Some(ExecutionId::new("second"))),
            Some(ExecutionId::new("second"))
        );
        assert_eq!(
            select_visible_execution(
                &visible,
                Some(ExecutionId::new("first")),
                Some(ExecutionId::new("second"))
            ),
            Some(ExecutionId::new("first"))
        );
        assert_eq!(
            select_visible_execution(&[], Some(ExecutionId::new("foreign")), None),
            None
        );
    }
}
