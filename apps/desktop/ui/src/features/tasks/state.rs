use crate::{
    client_adapter::{ClientWorker, ResponseGate},
    components::ScrollState,
    theme,
};
use gpui::{Context, Timer};
use lapis_desktop_client::TaskSession;
use lapis_editor_core::{ExecutionId, WorkspaceId};
use lapis_task::{ExecutionStatus, TaskControl, TaskMode, TaskSnapshot};
use std::time::Duration;
pub(crate) struct TasksFeature {
    worker: ClientWorker<TaskSession>,
    pub(super) records: Vec<TaskSnapshot>,
    pub(crate) selected_execution: Option<ExecutionId>,
    workspace: Option<WorkspaceId>,
    conversation: Option<lapis_editor_core::ConversationId>,
    pub(super) selected_text: Option<String>,
    pending: usize,
    gate: ResponseGate,
    pub(super) message: String,
    pub(super) scroll_states: TaskScrollStates,
}
pub(super) struct TaskScrollStates {
    pub(super) task_list: ScrollState,
    pub(super) task_events: ScrollState,
}
impl TasksFeature {
    pub(crate) fn new(session: TaskSession, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(250)).await;
                if this
                    .update(cx, |tasks, cx| {
                        if tasks.pending == 0 {
                            tasks.refresh(cx);
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
            records: Vec::new(),
            selected_execution: None,
            workspace: None,
            conversation: None,
            selected_text: None,
            pending: 0,
            gate: ResponseGate::default(),
            message: String::new(),
            scroll_states: TaskScrollStates {
                task_list: ScrollState::new(),
                task_events: ScrollState::new(),
            },
        }
    }
    pub(crate) fn set_conversation(&mut self, conversation: lapis_editor_core::ConversationId) {
        if self.conversation.as_ref() != Some(&conversation) {
            self.gate.invalidate();
            self.selected_execution = None;
        }
        self.conversation = Some(conversation);
    }
    pub(crate) fn set_selected_text(&mut self, text: Option<String>) {
        self.selected_text = text;
    }
    pub(crate) fn set_workspace(&mut self, workspace: Option<WorkspaceId>, cx: &mut Context<Self>) {
        if self.workspace == workspace {
            return;
        }
        self.workspace = workspace;
        self.gate.invalidate();
        self.records.clear();
        self.selected_execution = None;
        self.refresh(cx);
        cx.notify();
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
        self.operation(|_, _| Ok(None), cx);
    }
    fn operation(
        &mut self,
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
                if !tasks.gate.accept(token) {
                    return;
                }
                match result {
                    Ok((records, selected)) => {
                        tasks.records = records;
                        tasks.selected_execution = selected
                            .or_else(|| tasks.selected_execution.clone())
                            .or_else(|| {
                                tasks
                                    .records
                                    .first()
                                    .map(|record| record.execution.id.clone())
                            });
                        tasks.message.clear();
                    }
                    Err(error) => tasks.message = error,
                }
                cx.notify();
            },
        );
        if let Err(error) = admitted {
            self.pending = self.pending.saturating_sub(1);
            self.message = error;
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
        self.selected_execution = Some(id);
        cx.notify();
    }
    pub(super) fn control_selected_task(&mut self, control: TaskControl, cx: &mut Context<Self>) {
        let Some(id) = self.selected_execution.clone() else {
            return;
        };
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
pub(super) fn task_status_color(status: ExecutionStatus) -> gpui::Rgba {
    match status {
        ExecutionStatus::Succeeded => theme::colors().positive_text,
        ExecutionStatus::Failed | ExecutionStatus::Cancelled => theme::colors().danger_text,
        ExecutionStatus::WaitingForInput | ExecutionStatus::WaitingForApproval => {
            theme::colors().warning_text
        }
        _ => theme::colors().info_text,
    }
}
