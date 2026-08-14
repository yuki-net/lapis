use std::time::{Duration, Instant};

use crate::{
    extension_ui::{FeatureRegistry, PanelPosition, ThemeId, ViewId},
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

const PANEL_ANIMATION_DURATION: Duration = Duration::from_millis(100);

#[derive(Clone, Debug)]
struct PanelTransition {
    from: f32,
    to: f32,
    started_at: Instant,
    generation: u64,
}

impl PanelTransition {
    fn progress(&self, now: Instant) -> f32 {
        (now.duration_since(self.started_at).as_secs_f32() / PANEL_ANIMATION_DURATION.as_secs_f32())
            .min(1.0)
    }

    fn is_active(&self, now: Instant) -> bool {
        self.progress(now) < 1.0
    }

    fn value(&self, now: Instant) -> f32 {
        let progress = ease_in_out(self.progress(now));
        self.from + (self.to - self.from) * progress
    }
}

fn ease_in_out(value: f32) -> f32 {
    if value < 0.5 {
        2.0 * value * value
    } else {
        1.0 - (-2.0 * value + 2.0).powi(2) / 2.0
    }
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
    transition: Option<PanelTransition>,
    pending_open: Option<bool>,
    next_generation: u64,
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
            transition: None,
            pending_open: None,
            next_generation: 0,
        }
    }

    pub fn activate(&mut self, tab: PanelTab) {
        self.activate_without_open(tab);
        self.open = true;
    }

    pub fn activate_without_open(&mut self, tab: PanelTab) {
        if !self.tabs.contains(&tab) {
            self.tabs.push(tab.clone());
        }
        self.active = Some(tab);
    }

    pub fn activate_tool(&mut self, view: ViewId) {
        self.activate(PanelTab::Tool(view));
    }

    pub fn activate_tool_without_open(&mut self, view: ViewId) {
        self.activate_without_open(PanelTab::Tool(view));
    }

    pub fn set_open_immediate(&mut self, open: bool) {
        self.open = open;
        self.transition = None;
        self.pending_open = None;
        self.next_generation = self.next_generation.wrapping_add(1);
    }

    pub fn request_open(&mut self, open: bool, now: Instant) -> Option<(u64, Duration)> {
        if self.position == PanelPosition::Main {
            self.set_open_immediate(true);
            return None;
        }

        if let Some(transition) = &self.transition
            && transition.is_active(now)
        {
            self.pending_open = Some(open);
            return None;
        }

        let from = self.effective_size(now);
        self.transition = None;
        self.pending_open = None;
        if self.open == open {
            return None;
        }

        self.open = open;
        self.next_generation = self.next_generation.wrapping_add(1);
        let generation = self.next_generation;
        self.transition = Some(PanelTransition {
            from,
            to: if open { self.size } else { 0.0 },
            started_at: now,
            generation,
        });
        Some((generation, PANEL_ANIMATION_DURATION))
    }

    pub fn complete_transition(
        &mut self,
        generation: u64,
        now: Instant,
    ) -> Option<(u64, Duration)> {
        let Some(transition) = &self.transition else {
            return None;
        };
        if transition.generation != generation || transition.is_active(now) {
            return None;
        }

        self.transition = None;
        let Some(open) = self.pending_open.take() else {
            return None;
        };
        self.request_open(open, now)
    }

    pub fn is_animating(&self, now: Instant) -> bool {
        self.transition
            .as_ref()
            .is_some_and(|transition| transition.is_active(now))
    }

    pub fn is_transitioning(&self) -> bool {
        self.transition.is_some()
    }

    pub fn is_visible(&self, now: Instant) -> bool {
        self.position == PanelPosition::Main
            || self.open
            || self.is_animating(now)
            || self.pending_open.is_some()
    }

    pub fn effective_size(&self, now: Instant) -> f32 {
        self.transition.as_ref().map_or_else(
            || if self.open { self.size } else { 0.0 },
            |transition| transition.value(now),
        )
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
    pub settings_menu_open: bool,
    pub settings_menu_anchor: gpui::Point<gpui::Pixels>,
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
            settings_menu_open: false,
            settings_menu_anchor: gpui::point(gpui::px(0.0), gpui::px(0.0)),
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
            theme::BOTTOM_PANEL_HEIGHT,
        );
        let (generation, duration) = bottom.request_open(false, start).unwrap();
        assert_eq!(bottom.effective_size(start), theme::BOTTOM_PANEL_HEIGHT);
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
