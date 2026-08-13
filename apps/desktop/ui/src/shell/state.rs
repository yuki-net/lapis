use crate::{
    extension_ui::{FeatureRegistry, PanelPosition, ViewId},
    features::id,
    theme,
};
use lapis_app_services::DocumentTab;
use lapis_editor_core::DocumentId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResizeTarget {
    Left,
    Right,
    Bottom,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PanelTab {
    Document(DocumentId),
    Tool(ViewId),
}

#[derive(Clone, Debug)]
pub(crate) struct DraggedPanelTab {
    pub source_panel: PanelPosition,
    pub tab: PanelTab,
}

impl PanelTab {
    pub(crate) fn tool(view: impl Into<ViewId>) -> Self {
        Self::Tool(view.into())
    }

    pub(crate) fn view_id(&self) -> Option<&ViewId> {
        match self {
            Self::Tool(view) => Some(view),
            Self::Document(_) => None,
        }
    }
}

/// 一つの位置に表示されるWindow。位置以外の振る舞いはすべて共通にする。
#[derive(Clone, Debug)]
pub(crate) struct PanelHost {
    pub position: PanelPosition,
    pub tabs: Vec<PanelTab>,
    pub active: Option<PanelTab>,
    pub open: bool,
    pub size: f32,
}

impl PanelHost {
    fn new(position: PanelPosition, tabs: Vec<PanelTab>, open: bool, size: f32) -> Self {
        let active = tabs.first().cloned();
        Self {
            position,
            tabs,
            active,
            open,
            size,
        }
    }

    pub fn activate(&mut self, tab: PanelTab) {
        if !self.tabs.contains(&tab) {
            self.tabs.push(tab.clone());
        }
        self.active = Some(tab);
        self.open = true;
    }

    pub fn activate_tool(&mut self, view: ViewId) {
        self.activate(PanelTab::Tool(view));
    }

    pub fn close(&mut self) {
        self.open = false;
        self.active = None;
    }

    pub fn remove(&mut self, tab: &PanelTab) -> bool {
        let Some(index) = self.tabs.iter().position(|candidate| candidate == tab) else {
            return false;
        };
        self.tabs.remove(index);
        if self.active.as_ref() == Some(tab) {
            self.active = self.tabs.get(index.saturating_sub(1)).cloned();
        }
        true
    }

    pub fn contains(&self, tab: &PanelTab) -> bool {
        self.tabs.contains(tab)
    }

    pub fn active_tool(&self) -> Option<&ViewId> {
        self.active.as_ref().and_then(PanelTab::view_id)
    }
}

#[derive(Clone)]
pub(crate) struct ShellState {
    pub main_panel: PanelHost,
    pub left_panel: PanelHost,
    pub bottom_panel: PanelHost,
    pub right_panel: PanelHost,
    pub command_palette_open: bool,
    pub tool_picker: Option<PanelPosition>,
    pub tool_picker_query: String,
    pub resizing: Option<ResizeTarget>,
}

impl Default for ShellState {
    fn default() -> Self {
        Self {
            main_panel: PanelHost::new(PanelPosition::Main, Vec::new(), true, 0.0),
            left_panel: PanelHost::new(
                PanelPosition::Left,
                vec![PanelTab::tool(id::VIEW_FILES)],
                true,
                theme::TOOL_ISLAND_WIDTH,
            ),
            bottom_panel: PanelHost::new(
                PanelPosition::Bottom,
                Vec::new(),
                false,
                theme::BOTTOM_PANEL_HEIGHT,
            ),
            right_panel: PanelHost::new(
                PanelPosition::Right,
                Vec::new(),
                false,
                theme::SIDE_PANEL_WIDTH,
            ),
            command_palette_open: false,
            tool_picker: None,
            tool_picker_query: String::new(),
            resizing: None,
        }
    }
}

impl ShellState {
    pub fn panel(&self, position: PanelPosition) -> &PanelHost {
        match position {
            PanelPosition::Main => &self.main_panel,
            PanelPosition::Left => &self.left_panel,
            PanelPosition::Bottom => &self.bottom_panel,
            PanelPosition::Right => &self.right_panel,
        }
    }

    pub fn panel_mut(&mut self, position: PanelPosition) -> &mut PanelHost {
        match position {
            PanelPosition::Main => &mut self.main_panel,
            PanelPosition::Left => &mut self.left_panel,
            PanelPosition::Bottom => &mut self.bottom_panel,
            PanelPosition::Right => &mut self.right_panel,
        }
    }

    pub fn panels(&self) -> [&PanelHost; 4] {
        [
            &self.main_panel,
            &self.left_panel,
            &self.bottom_panel,
            &self.right_panel,
        ]
    }

    pub fn panels_mut(&mut self) -> [&mut PanelHost; 4] {
        [
            &mut self.main_panel,
            &mut self.left_panel,
            &mut self.bottom_panel,
            &mut self.right_panel,
        ]
    }

    pub fn activate_view(&mut self, position: PanelPosition, view: ViewId) {
        self.panel_mut(position).activate_tool(view);
    }

    pub fn set_tool_picker_query(&mut self, query: impl Into<String>) {
        self.tool_picker_query = query.into();
    }

    pub fn synchronize_documents(&mut self, documents: &[DocumentTab]) {
        let valid = documents
            .iter()
            .map(|document| &document.id)
            .collect::<Vec<_>>();
        for panel in self.panels_mut() {
            panel.tabs.retain(|tab| match tab {
                PanelTab::Document(id) => valid.contains(&id),
                PanelTab::Tool(_) => true,
            });
            if let Some(active) = &panel.active
                && !panel.tabs.contains(active)
            {
                panel.active = panel.tabs.first().cloned();
            }
        }
        for document in documents {
            let tab = PanelTab::Document(document.id.clone());
            if !self.panels().iter().any(|panel| panel.contains(&tab)) {
                self.main_panel.tabs.push(tab.clone());
            }
            if document.active {
                for panel in self.panels_mut() {
                    if panel.contains(&tab) {
                        panel.active = Some(tab.clone());
                    }
                }
            }
        }
    }

    pub fn move_tab(&mut self, source: PanelPosition, target: PanelPosition, tab: PanelTab) {
        if source == target {
            self.panel_mut(target).activate(tab);
            return;
        }
        self.panel_mut(source).remove(&tab);
        self.panel_mut(target).activate(tab);
    }

    pub fn synchronize_activation(&self, registry: &mut FeatureRegistry) {
        for panel in self.panels() {
            registry.set_panel_active_view(
                panel.position,
                panel
                    .open
                    .then(|| panel.active.as_ref().and_then(PanelTab::view_id).cloned())
                    .flatten(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initializes_four_panels_with_main_visible() {
        let state = ShellState::default();

        assert_eq!(state.panels().len(), 4);
        assert!(state.main_panel.open);
        assert!(state.left_panel.open);
        assert!(!state.bottom_panel.open);
        assert!(!state.right_panel.open);
        assert_eq!(
            state.left_panel.active_tool().map(ViewId::as_str),
            Some(id::VIEW_FILES)
        );
    }

    #[test]
    fn moving_a_tab_removes_it_from_source_and_activates_target() {
        let mut state = ShellState::default();
        let tab = PanelTab::tool(id::VIEW_FILES);

        state.move_tab(PanelPosition::Left, PanelPosition::Main, tab.clone());

        assert!(!state.left_panel.contains(&tab));
        assert!(state.main_panel.contains(&tab));
        assert_eq!(state.main_panel.active, Some(tab));
    }

    #[test]
    fn moving_a_tab_to_closed_panel_does_not_change_state_at_action_boundary() {
        let mut state = ShellState::default();
        let tab = PanelTab::tool(id::VIEW_FILES);

        state.move_tab(PanelPosition::Left, PanelPosition::Right, tab.clone());

        assert!(!state.left_panel.contains(&tab));
        assert!(state.right_panel.contains(&tab));
    }
}
