use lapis_app_services::DocumentTab;

use crate::{
    extension_ui::{FeatureRegistry, PanelPosition, ThemeId, ViewId},
    features::id,
    tokens,
};

use super::{panel::PanelHost, tab_state::PanelTab};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HeaderMenuSection {
    File,
    Edit,
    View,
    Window,
    Help,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResizeTarget {
    Left,
    Right,
    Bottom,
}

#[derive(Clone)]
pub(crate) struct ShellState {
    pub main_panel: PanelHost,
    pub left_panel: PanelHost,
    pub bottom_panel: PanelHost,
    pub right_panel: PanelHost,
    pub command_palette_open: bool,
    pub tool_picker: Option<PanelPosition>,
    pub tool_picker_anchor: gpui::Point<gpui::Pixels>,
    pub tool_picker_query: String,
    pub settings_menu_open: bool,
    pub settings_menu_anchor: gpui::Point<gpui::Pixels>,
    pub header_menu_open: bool,
    pub header_menu_anchor: gpui::Point<gpui::Pixels>,
    pub header_menu_section: Option<HeaderMenuSection>,
    pub theme_picker_open: bool,
    pub theme_save_in_flight: bool,
    pub theme_before_save: Option<ThemeId>,
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
                f32::from(tokens::size::TOOL_ISLAND_WIDTH),
            ),
            bottom_panel: PanelHost::new(
                PanelPosition::Bottom,
                Vec::new(),
                false,
                f32::from(tokens::size::BOTTOM_PANEL_HEIGHT),
            ),
            right_panel: PanelHost::new(
                PanelPosition::Right,
                Vec::new(),
                false,
                f32::from(tokens::size::SIDE_PANEL_WIDTH),
            ),
            command_palette_open: false,
            tool_picker: None,
            tool_picker_anchor: gpui::point(gpui::px(120.0), gpui::px(82.0)),
            tool_picker_query: String::new(),
            settings_menu_open: false,
            settings_menu_anchor: gpui::point(gpui::px(0.0), gpui::px(0.0)),
            header_menu_open: false,
            header_menu_anchor: gpui::point(gpui::px(0.0), gpui::px(0.0)),
            header_menu_section: None,
            theme_picker_open: false,
            theme_save_in_flight: false,
            theme_before_save: None,
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
    use std::time::{Duration, Instant};

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

    #[test]
    fn removing_tabs_keeps_panel_open_and_selects_adjacent_tabs() {
        let mut state = ShellState::default();
        state.main_panel.activate_tool(ViewId::new(id::VIEW_FILES));
        state
            .main_panel
            .activate_tool(ViewId::new(id::VIEW_TERMINAL));
        let files = PanelTab::tool(id::VIEW_FILES);
        let terminal = PanelTab::tool(id::VIEW_TERMINAL);

        assert!(state.main_panel.remove(&files));
        assert_eq!(state.main_panel.active, Some(terminal.clone()));
        assert!(state.main_panel.remove(&terminal));
        assert!(state.main_panel.open);
        assert!(state.main_panel.tabs.is_empty());
        assert_eq!(state.main_panel.active, None);
    }

    #[test]
    fn panel_transition_interpolates_width_and_height_endpoints() {
        let start = Instant::now();
        let mut left = PanelHost::new(PanelPosition::Left, Vec::new(), false, 260.0);
        let (generation, duration) = left.request_open(true, start).unwrap();

        assert_eq!(left.effective_size(start), 0.0);
        assert!(left.effective_size(start + duration / 2) > 0.0);
        assert_eq!(left.effective_size(start + duration), 260.0);
        assert!(
            left.complete_transition(generation, start + duration)
                .is_none()
        );

        let mut bottom = PanelHost::new(
            PanelPosition::Bottom,
            Vec::new(),
            true,
            f32::from(tokens::size::BOTTOM_PANEL_HEIGHT),
        );
        let (generation, duration) = bottom.request_open(false, start).unwrap();
        assert_eq!(
            bottom.effective_size(start),
            f32::from(tokens::size::BOTTOM_PANEL_HEIGHT)
        );
        assert_eq!(bottom.effective_size(start + duration), 0.0);
        assert!(
            bottom
                .complete_transition(generation, start + duration)
                .is_none()
        );
    }

    #[test]
    fn panel_transition_applies_the_last_requested_state_after_completion() {
        let start = Instant::now();
        let mut panel = PanelHost::new(PanelPosition::Right, Vec::new(), false, 310.0);
        let (generation, duration) = panel.request_open(true, start).unwrap();

        assert!(panel.request_open(false, start + duration / 2).is_none());
        let next = panel.complete_transition(generation, start + duration);
        assert!(next.is_some());
        assert!(!panel.open);

        let (next_generation, next_duration) = next.unwrap();
        assert_eq!(panel.effective_size(start + duration), 310.0);
        assert_eq!(panel.effective_size(start + duration + next_duration), 0.0);
        assert!(
            panel
                .complete_transition(next_generation, start + duration + next_duration)
                .is_none()
        );
    }

    #[test]
    fn restoring_panel_state_clears_pending_animation() {
        let start = Instant::now();
        let mut panel = PanelHost::new(PanelPosition::Left, Vec::new(), false, 260.0);
        let _ = panel.request_open(true, start);
        panel.set_open_immediate(false);

        assert!(!panel.is_animating(start + Duration::from_millis(100)));
        assert!(!panel.is_visible(start + Duration::from_millis(100)));
        assert_eq!(
            panel.effective_size(start + Duration::from_millis(100)),
            0.0
        );
    }
}
