use super::*;
use crate::features::conversation::{apply_view_state, capture_view_state};

impl Editor {
    pub(super) fn conversation_view_state(&self) -> ConversationViewState {
        capture_view_state(&self.shell)
    }

    pub(super) fn apply_conversation_view(&mut self, view: ConversationViewState) {
        let documents = self.session.tabs();
        apply_view_state(view, &mut self.shell, &documents, &self.feature_registry);
        self.refresh_feature_activation();
    }

    pub(super) fn capture_conversation(&mut self) -> Result<(), lapis_workspace::WorkspaceError> {
        let view = self.conversation_view_state();
        self.conversation.session.capture(
            &self.session,
            view,
            self.tasks.selected_execution.clone(),
            self.terminal.session.terminals(),
        )
    }

    #[allow(dead_code)]
    pub(super) fn create_conversation(&mut self, cx: &mut Context<Self>) {
        if let Err(error) = self.capture_conversation() {
            self.status = format!("Conversation 保存失敗: {error}");
            cx.notify();
            return;
        }
        if let Err(error) = self.terminal.session.terminate_all() {
            self.status = format!("Terminal 終了失敗: {error}");
            cx.notify();
            return;
        }
        self.terminal.session.restore_summaries(&[]);
        match self
            .conversation
            .session
            .create(&self.session, ConversationViewState::default())
        {
            Ok(_) => {
                self.apply_conversation_view(ConversationViewState::default());
                self.status = "Conversation を作成しました".to_owned();
            }
            Err(error) => self.status = format!("Conversation 作成失敗: {error}"),
        }
        cx.notify();
    }

    #[allow(dead_code)]
    pub(super) fn switch_conversation(
        &mut self,
        id: lapis_editor_core::ConversationId,
        cx: &mut Context<Self>,
    ) {
        if self.conversation.session.active_id() == &id {
            return;
        }
        if let Err(error) = self.capture_conversation() {
            self.status = format!("Conversation 保存失敗: {error}");
            cx.notify();
            return;
        }
        if let Err(error) = self.terminal.session.terminate_all() {
            self.status = format!("Terminal 終了失敗: {error}");
            cx.notify();
            return;
        }
        match self.conversation.session.switch(&id, &mut self.session) {
            Ok(view) => {
                self.apply_conversation_view(view);
                let record = self.conversation.session.active_record().cloned();
                self.tasks.selected_execution = record
                    .as_ref()
                    .and_then(|record| record.selected_execution.clone());
                self.terminal
                    .session
                    .restore_summaries(&record.map(|record| record.terminals).unwrap_or_default());
                self.restore_active_view();
                self.status = "Conversation を切り替えました".to_owned();
            }
            Err(error) => self.status = format!("Conversation 切り替え失敗: {error}"),
        }
        cx.notify();
    }
}
