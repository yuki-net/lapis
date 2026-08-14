use super::*;
use crate::extension_ui::PanelPosition;
use crate::shell::PanelTab;
use lapis_app_services::PanelViewState;

impl Editor {
    pub(super) fn conversation_view_state(&self) -> ConversationViewState {
        ConversationViewState {
            panels: self
                .shell
                .panels()
                .into_iter()
                .map(|panel| PanelViewState {
                    position: panel_position_name(panel.position).to_owned(),
                    tabs: panel.tabs.iter().map(serialize_panel_tab).collect(),
                    active_tab: panel.active.as_ref().map(serialize_panel_tab),
                    open: panel.open,
                    size: panel.size,
                })
                .collect(),
            active_tool: self
                .shell
                .left_panel
                .active_tool()
                .map_or_else(|| "files".to_owned(), |view| view.as_str().to_owned()),
            side_panel: self
                .shell
                .right_panel
                .active_tool()
                .map(|view| view.as_str().to_owned()),
            bottom_panel: self
                .shell
                .bottom_panel
                .active_tool()
                .map(|view| view.as_str().to_owned()),
            tool_width: self.shell.left_panel.size,
            side_width: self.shell.right_panel.size,
            bottom_height: self.shell.bottom_panel.size,
        }
    }

    pub(super) fn apply_conversation_view(&mut self, view: ConversationViewState) {
        if !view.panels.is_empty() {
            for panel_view in view.panels {
                let Some(position) = panel_position(panel_view.position.as_str()) else {
                    continue;
                };
                let valid_documents = self
                    .session
                    .tabs()
                    .into_iter()
                    .map(|document| document.id)
                    .collect::<Vec<_>>();
                let valid_tools = self
                    .feature_registry
                    .panel_contributions(position)
                    .into_iter()
                    .filter_map(|contribution| contribution.view.clone())
                    .collect::<Vec<_>>();
                let panel = self.shell.panel_mut(position);
                panel.tabs = panel_view
                    .tabs
                    .into_iter()
                    .filter_map(deserialize_panel_tab)
                    .filter(|tab| match tab {
                        PanelTab::Document(id) => valid_documents.contains(id),
                        PanelTab::Tool(view) => valid_tools.contains(view),
                    })
                    .collect();
                panel.active = panel_view
                    .active_tab
                    .and_then(deserialize_panel_tab)
                    .filter(|tab| match tab {
                        PanelTab::Document(id) => valid_documents.contains(id),
                        PanelTab::Tool(view) => valid_tools.contains(view),
                    });
                if panel.active.is_none() {
                    panel.active = panel.tabs.first().cloned();
                }
                panel.set_open_immediate(position == PanelPosition::Main || panel_view.open);
                panel.size = panel_size(position, panel_view.size);
            }
            self.shell.main_panel.set_open_immediate(true);
            self.refresh_feature_activation();
            return;
        }

        self.shell
            .left_panel
            .activate_tool(ViewId::new(match view.active_tool.as_str() {
                "search" | id::VIEW_SEARCH => id::VIEW_SEARCH,
                "git" | id::VIEW_GIT => id::VIEW_GIT,
                "history" | id::VIEW_HISTORY => id::VIEW_HISTORY,
                _ => id::VIEW_FILES,
            }));
        self.shell.left_panel.size = view.tool_width.clamp(190.0, 380.0);
        self.shell.right_panel.size = view.side_width.clamp(260.0, 480.0);
        self.shell.bottom_panel.size = view.bottom_height.clamp(140.0, 360.0);
        if let Some(side) = view.side_panel {
            self.shell.right_panel.activate_tool(ViewId::new(side));
        } else {
            self.shell.right_panel.set_open_immediate(false);
        }
        if let Some(bottom) = view.bottom_panel {
            self.shell.bottom_panel.activate_tool(ViewId::new(bottom));
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
            Err(error) => self.status = format!("Conversation 切替失敗: {error}"),
        }
        cx.notify();
    }
}

fn serialize_panel_tab(tab: &PanelTab) -> String {
    match tab {
        PanelTab::Document(id) => format!("document:{}", id.as_str()),
        PanelTab::Tool(view) => format!("tool:{}", view.as_str()),
    }
}

fn deserialize_panel_tab(value: String) -> Option<PanelTab> {
    if let Some(id) = value.strip_prefix("document:") {
        return Some(PanelTab::Document(lapis_editor_core::DocumentId::new(id)));
    }
    Some(PanelTab::Tool(ViewId::new(
        value.strip_prefix("tool:").unwrap_or(&value),
    )))
}

fn panel_position(value: &str) -> Option<PanelPosition> {
    match value {
        "main-panel" | "center-panel" => Some(PanelPosition::Main),
        "left-panel" => Some(PanelPosition::Left),
        "bottom-panel" => Some(PanelPosition::Bottom),
        "right-panel" => Some(PanelPosition::Right),
        _ => None,
    }
}

fn panel_position_name(position: PanelPosition) -> &'static str {
    match position {
        PanelPosition::Main => "main-panel",
        PanelPosition::Left => "left-panel",
        PanelPosition::Bottom => "bottom-panel",
        PanelPosition::Right => "right-panel",
    }
}

fn panel_size(position: PanelPosition, value: f32) -> f32 {
    match position {
        PanelPosition::Main => 0.0,
        PanelPosition::Left => value.clamp(190.0, 380.0),
        PanelPosition::Bottom => value.clamp(140.0, 360.0),
        PanelPosition::Right => value.clamp(260.0, 480.0),
    }
}
