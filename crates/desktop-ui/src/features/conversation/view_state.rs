use super::*;

impl Editor {
    pub(super) fn conversation_view_state(&self) -> ConversationViewState {
        ConversationViewState {
            active_tool: self.shell.active_tool.as_str().to_owned(),
            side_panel: self
                .shell
                .side_panel
                .as_ref()
                .map(|panel| panel.as_str().to_owned()),
            bottom_panel: self
                .shell
                .bottom_panel_open
                .then(|| self.shell.bottom_panel.as_str().to_owned()),
            tool_width: self.shell.tool_island_width,
            side_width: self.shell.side_panel_width,
            bottom_height: self.shell.bottom_panel_height,
        }
    }

    pub(super) fn apply_conversation_view(&mut self, view: ConversationViewState) {
        self.shell.active_tool = ViewId::new(match view.active_tool.as_str() {
            "search" | id::VIEW_SEARCH => id::VIEW_SEARCH,
            "git" | id::VIEW_GIT => id::VIEW_GIT,
            "history" | id::VIEW_HISTORY => id::VIEW_HISTORY,
            _ => id::VIEW_FILES,
        });
        self.shell.side_panel = match view.side_panel.as_deref() {
            Some("preview") | Some(id::VIEW_PREVIEW) => Some(ViewId::new(id::VIEW_PREVIEW)),
            Some("assistant") | Some(id::VIEW_ASSISTANT) => Some(ViewId::new(id::VIEW_ASSISTANT)),
            _ => None,
        };
        self.shell.bottom_panel_open = view.bottom_panel.is_some();
        self.shell.bottom_panel = ViewId::new(match view.bottom_panel.as_deref() {
            Some("problems") | Some(id::VIEW_PROBLEMS) => id::VIEW_PROBLEMS,
            Some("output") | Some(id::VIEW_OUTPUT) => id::VIEW_OUTPUT,
            _ => id::VIEW_TERMINAL,
        });
        self.shell.tool_island_width = view.tool_width.clamp(190.0, 380.0);
        self.shell.side_panel_width = view.side_width.clamp(260.0, 480.0);
        self.shell.bottom_panel_height = view.bottom_height.clamp(140.0, 360.0);
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
            Err(error) => self.status = format!("Conversation 切替失敗: {error}"),
        }
        cx.notify();
    }
}
