use super::*;
use crate::extension_ui::PanelPosition;
use lapis_app_services::PanelViewState;

impl Editor {
    pub(super) fn conversation_view_state(&self) -> ConversationViewState {
        ConversationViewState {
            panels: [
                &self.shell.left_panel,
                &self.shell.bottom_panel,
                &self.shell.right_panel,
            ]
            .into_iter()
            .map(|panel| PanelViewState {
                position: panel_position_name(panel.position).to_owned(),
                tabs: panel
                    .tabs
                    .iter()
                    .map(|tab| tab.as_str().to_owned())
                    .collect(),
                active_tab: panel.active.as_ref().map(|tab| tab.as_str().to_owned()),
                open: panel.open,
                size: panel.size,
            })
            .collect(),
            active_tool: self
                .shell
                .left_panel
                .active
                .as_ref()
                .map_or_else(|| "files".to_owned(), |view| view.as_str().to_owned()),
            side_panel: self
                .shell
                .right_panel
                .active
                .as_ref()
                .map(|view| view.as_str().to_owned()),
            bottom_panel: self
                .shell
                .bottom_panel
                .active
                .as_ref()
                .map(|view| view.as_str().to_owned()),
            tool_width: self.shell.left_panel.size,
            side_width: self.shell.right_panel.size,
            bottom_height: self.shell.bottom_panel.size,
        }
    }

    pub(super) fn apply_conversation_view(&mut self, view: ConversationViewState) {
        if !view.panels.is_empty() {
            for panel_view in view.panels {
                let position = match panel_view.position.as_str() {
                    "left-panel" => PanelPosition::Left,
                    "bottom-panel" => PanelPosition::Bottom,
                    "right-panel" => PanelPosition::Right,
                    _ => continue,
                };
                if let Some(panel) = self.shell.panel_mut(position) {
                    panel.tabs = panel_view.tabs.into_iter().map(ViewId::new).collect();
                    panel.active = panel_view.active_tab.map(ViewId::new);
                    panel.open = panel_view.open;
                    panel.size = match position {
                        PanelPosition::Left => panel_view.size.clamp(190.0, 380.0),
                        PanelPosition::Bottom => panel_view.size.clamp(140.0, 360.0),
                        PanelPosition::Right => panel_view.size.clamp(260.0, 480.0),
                        PanelPosition::Center => panel_view.size,
                    };
                }
            }
            self.refresh_feature_activation();
            return;
        }
        self.shell
            .left_panel
            .activate(ViewId::new(match view.active_tool.as_str() {
                "search" | id::VIEW_SEARCH => id::VIEW_SEARCH,
                "git" | id::VIEW_GIT => id::VIEW_GIT,
                "history" | id::VIEW_HISTORY => id::VIEW_HISTORY,
                _ => id::VIEW_FILES,
            }));
        self.shell.left_panel.size = view.tool_width.clamp(190.0, 380.0);
        self.shell.right_panel.size = view.side_width.clamp(260.0, 480.0);
        self.shell.bottom_panel.size = view.bottom_height.clamp(140.0, 360.0);
        if let Some(side) = view.side_panel {
            self.shell.right_panel.activate(ViewId::new(side));
        } else {
            self.shell.right_panel.close();
        }
        if let Some(bottom) = view.bottom_panel {
            self.shell.bottom_panel.activate(ViewId::new(bottom));
        }
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

    #[allow(dead_code)] // 会話操作 UI の移設まで、新規作成処理を保持する。
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

    #[allow(dead_code)] // 会話一覧 UI の移設まで、復元用の操作を保持する。
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

fn panel_position_name(position: PanelPosition) -> &'static str {
    match position {
        PanelPosition::Left => "left-panel",
        PanelPosition::Center => "center-panel",
        PanelPosition::Bottom => "bottom-panel",
        PanelPosition::Right => "right-panel",
    }
}
