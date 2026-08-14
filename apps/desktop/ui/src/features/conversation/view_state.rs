use crate::{
    extension_ui::{FeatureRegistry, PanelPosition, ViewId},
    features::id,
    shell::{PanelTab, ShellState},
};
use lapis_app_services::{ConversationViewState, DocumentTab, PanelViewState};

pub(crate) fn capture_view_state(shell: &ShellState) -> ConversationViewState {
    ConversationViewState {
        panels: shell
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
        active_tool: shell
            .left_panel
            .active_tool()
            .map_or_else(|| "files".to_owned(), |view| view.as_str().to_owned()),
        side_panel: shell
            .right_panel
            .active_tool()
            .map(|view| view.as_str().to_owned()),
        bottom_panel: shell
            .bottom_panel
            .active_tool()
            .map(|view| view.as_str().to_owned()),
        tool_width: shell.left_panel.size,
        side_width: shell.right_panel.size,
        bottom_height: shell.bottom_panel.size,
    }
}

pub(crate) fn apply_view_state(
    view: ConversationViewState,
    shell: &mut ShellState,
    documents: &[DocumentTab],
    registry: &FeatureRegistry,
) {
    if !view.panels.is_empty() {
        let valid_documents = documents
            .iter()
            .map(|document| document.id.clone())
            .collect::<Vec<_>>();
        for panel_view in view.panels {
            let Some(position) = panel_position(panel_view.position.as_str()) else {
                continue;
            };
            let valid_tools = registry
                .panel_contributions(position)
                .into_iter()
                .filter_map(|contribution| contribution.view.clone())
                .collect::<Vec<_>>();
            let panel = shell.panel_mut(position);
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
        shell.main_panel.set_open_immediate(true);
        return;
    }

    shell
        .left_panel
        .activate_tool(ViewId::new(match view.active_tool.as_str() {
            "search" | id::VIEW_SEARCH => id::VIEW_SEARCH,
            "git" | id::VIEW_GIT => id::VIEW_GIT,
            "history" | id::VIEW_HISTORY => id::VIEW_HISTORY,
            _ => id::VIEW_FILES,
        }));
    shell.left_panel.size = view.tool_width.clamp(190.0, 380.0);
    shell.right_panel.size = view.side_width.clamp(260.0, 480.0);
    shell.bottom_panel.size = view.bottom_height.clamp(140.0, 360.0);
    if let Some(side) = view.side_panel {
        shell.right_panel.activate_tool(ViewId::new(side));
    } else {
        shell.right_panel.set_open_immediate(false);
    }
    if let Some(bottom) = view.bottom_panel {
        shell.bottom_panel.activate_tool(ViewId::new(bottom));
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
