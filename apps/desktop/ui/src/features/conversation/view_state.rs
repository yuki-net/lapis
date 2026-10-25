use crate::shell::panel_size;
use crate::{
    extension_ui::{FeatureRegistry, PanelPosition, ViewId},
    features::id,
    shell::{PanelTab, ShellState},
};
use lapis_desktop_client::{ConversationViewState, DocumentTab, PanelViewState};

pub(crate) fn capture_view_state(shell: &ShellState) -> ConversationViewState {
    ConversationViewState {
        layout: Some(shell.layout.capture_view_state()),
        show_panel_controls: shell.show_panel_controls,
        workbench: Default::default(),
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
            .panel(PanelPosition::Left)
            .active_tool()
            .map_or_else(|| "files".to_owned(), |view| view.as_str().to_owned()),
        side_panel: shell
            .panel(PanelPosition::Right)
            .active_tool()
            .map(|view| view.as_str().to_owned()),
        bottom_panel: shell
            .panel(PanelPosition::Bottom)
            .active_tool()
            .map(|view| view.as_str().to_owned()),
        tool_width: shell.panel(PanelPosition::Left).size,
        side_width: shell.panel(PanelPosition::Right).size,
        bottom_height: shell.panel(PanelPosition::Bottom).size,
    }
}

pub(crate) fn apply_view_state(
    view: ConversationViewState,
    shell: &mut ShellState,
    documents: &[DocumentTab],
    registry: &FeatureRegistry,
) {
    if let Some(layout) = view.layout.as_ref()
        && shell.layout.restore_view_state(layout, |raw| {
            let tab = deserialize_panel_tab(raw)?;
            match &tab {
                PanelTab::Document(id) => documents.iter().any(|document| &document.id == id),
                PanelTab::Tool { view, .. } => [
                    PanelPosition::Main,
                    PanelPosition::Left,
                    PanelPosition::Bottom,
                    PanelPosition::Right,
                ]
                .into_iter()
                .flat_map(|position| registry.panel_contributions(position))
                .any(|contribution| contribution.view.as_ref() == Some(view)),
            }
            .then_some(tab)
        })
    {
        shell.show_panel_controls = view.show_panel_controls;
        shell
            .panel_mut(PanelPosition::Main)
            .set_open_immediate(true);
        shell.synchronize_documents(documents);
        return;
    }
    // 拒否した候補や以前のTaskの可変Groupを旧形式の復元へ混ぜない。
    shell.layout = Default::default();
    if !view.panels.is_empty() {
        shell.show_panel_controls = view.show_panel_controls;
        let valid_documents = documents
            .iter()
            .map(|document| document.id.clone())
            .collect::<Vec<_>>();
        let mut seen_instances = std::collections::HashSet::new();
        let mut restored_positions = std::collections::HashSet::new();
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
            if restored_positions.insert(position) {
                panel.tabs.clear();
            }
            let tabs = panel_view
                .tabs
                .into_iter()
                .filter_map(|value| deserialize_panel_tab(&value))
                .filter(|tab| match tab {
                    PanelTab::Document(id) => valid_documents.contains(id),
                    PanelTab::Tool { view, .. } => valid_tools.contains(view),
                })
                .filter(|tab| seen_instances.insert(tab.instance_id()))
                .collect::<Vec<_>>();
            panel.tabs.extend(tabs);
            panel.active = panel_view
                .active_tab
                .as_deref()
                .and_then(deserialize_panel_tab)
                .filter(|tab| match tab {
                    PanelTab::Document(id) => valid_documents.contains(id),
                    PanelTab::Tool { view, .. } => valid_tools.contains(view),
                })
                .filter(|tab| panel.tabs.contains(tab));
            if panel.active.is_none() {
                panel.active = panel.tabs.first().cloned();
            }
            panel.set_open_immediate(position == PanelPosition::Main || panel_view.open);
            panel.size = panel_size(position, panel_view.size);
        }
        shell
            .panel_mut(PanelPosition::Main)
            .set_open_immediate(true);
        shell.synchronize_documents(documents);
        return;
    }

    shell.show_panel_controls = view.show_panel_controls;
    shell
        .panel_mut(PanelPosition::Left)
        .activate_tool(ViewId::new(match view.active_tool.as_str() {
            "search" | id::VIEW_SEARCH => id::VIEW_SEARCH,
            "git" | id::VIEW_GIT => id::VIEW_GIT,
            "history" | "projects" | "lapis.view.history" | id::VIEW_PROJECT_TASKS => {
                id::VIEW_PROJECT_TASKS
            }
            _ => id::VIEW_FILES,
        }));
    shell.panel_mut(PanelPosition::Left).size = panel_size(PanelPosition::Left, view.tool_width);
    shell.panel_mut(PanelPosition::Right).size = panel_size(PanelPosition::Right, view.side_width);
    shell.panel_mut(PanelPosition::Bottom).size =
        panel_size(PanelPosition::Bottom, view.bottom_height);
    if let Some(side) = view.side_panel {
        shell
            .panel_mut(PanelPosition::Right)
            .activate_tool(ViewId::new(side));
    } else {
        shell
            .panel_mut(PanelPosition::Right)
            .set_open_immediate(false);
    }
    if let Some(bottom) = view.bottom_panel {
        shell
            .panel_mut(PanelPosition::Bottom)
            .activate_tool(ViewId::new(bottom));
    }
    shell.synchronize_documents(documents);
}

fn serialize_panel_tab(tab: &PanelTab) -> String {
    match tab {
        PanelTab::Document(id) => format!("document:{}", id.as_str()),
        PanelTab::Tool { view, instance } => {
            format!("tool:{}:{}", view.as_str(), instance.as_str())
        }
    }
}

fn deserialize_panel_tab(value: &str) -> Option<PanelTab> {
    if let Some(id) = value.strip_prefix("document:") {
        return Some(PanelTab::Document(lapis_editor_core::DocumentId::new(id)));
    }
    let value = value.strip_prefix("tool:").unwrap_or(value);
    if let Some((view, instance)) = value.split_once(':') {
        return Some(PanelTab::tool_instance(
            ViewId::new(view),
            instance.to_owned(),
        ));
    }
    Some(PanelTab::tool(ViewId::new(value)))
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

#[cfg(test)]
mod tests {
    use super::*;
    use lapis_desktop_client::{LayoutGroupViewState, LayoutNodeViewState};

    #[test]
    fn broken_layout_falls_back_without_old_groups_or_lost_dirty_documents() {
        let mut shell = ShellState::default();
        let mut previous = shell.layout.capture_view_state();
        previous.groups.push(LayoutGroupViewState {
            id: "group-5".into(),
            position: "main-panel".into(),
            tabs: vec![],
            active_tab: None,
            open: true,
            zone_hidden: false,
            size: 0.0,
        });
        previous.root = LayoutNodeViewState::Split {
            direction: "horizontal".into(),
            ratio: 0.5,
            first: Box::new(previous.root),
            second: Box::new(LayoutNodeViewState::Tabs {
                group: "group-5".into(),
            }),
        };
        assert!(
            shell
                .layout
                .restore_view_state(&previous, deserialize_panel_tab)
        );
        let mut broken = previous;
        broken.groups.retain(|group| group.position == "main-panel");
        let document = DocumentTab {
            id: lapis_editor_core::DocumentId::new("unsaved"),
            display_name: "Untitled".into(),
            path: None,
            dirty: true,
            external_change: lapis_document::ExternalChange::Unchanged,
            encoding: Default::default(),
            active: true,
        };
        let view = ConversationViewState {
            layout: Some(broken),
            panels: vec![PanelViewState {
                position: "left-panel".into(),
                tabs: vec![format!("tool:{}", id::VIEW_FILES)],
                active_tab: Some("tool:unknown".into()),
                open: true,
                size: f32::NAN,
            }],
            ..Default::default()
        };
        apply_view_state(
            view,
            &mut shell,
            std::slice::from_ref(&document),
            &crate::features::bundled_registry(),
        );
        assert_eq!(shell.layout.leaf_group_ids().len(), 4);
        assert!(shell.layout.validate().is_ok());
        assert_eq!(
            shell
                .layout
                .placements()
                .iter()
                .filter(|(tab, _)| tab == &PanelTab::Document(document.id.clone()))
                .count(),
            1
        );
        assert!(document.dirty);
        assert_eq!(
            shell.panel(PanelPosition::Main).active,
            Some(PanelTab::Document(document.id))
        );
    }
}
