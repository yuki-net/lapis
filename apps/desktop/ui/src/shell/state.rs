use std::time::{Duration, Instant};

use lapis_app_services::DocumentTab;

use crate::{
    extension_ui::{FeatureRegistry, PanelPosition, ThemeId, ViewId},
    features::id,
    tokens,
};

use super::{panel::PanelHost, tab_state::PanelTab};

const PANEL_SPAN_ANIMATION_DURATION: Duration = Duration::from_millis(160);

#[derive(Clone, Debug)]
pub(crate) struct PanelSpanTransition {
    from_side: f32,
    from_bottom: f32,
    target_open: bool,
    started_at: Instant,
}

impl PanelSpanTransition {
    pub(crate) fn from_visual(
        from_side: f32,
        from_bottom: f32,
        target_open: bool,
        started_at: Instant,
    ) -> Self {
        Self {
            from_side,
            from_bottom,
            target_open,
            started_at,
        }
    }

    fn progress(&self, now: Instant) -> f32 {
        (now.duration_since(self.started_at).as_secs_f32()
            / PANEL_SPAN_ANIMATION_DURATION.as_secs_f32())
        .min(1.0)
    }

    fn ease(progress: f32) -> f32 {
        1.0 - (1.0 - progress).powi(3)
    }

    fn phase_split(&self) -> f32 {
        let (first_distance, second_distance) = if self.target_open {
            (1.0 - self.from_side, 1.0 - self.from_bottom)
        } else {
            (self.from_bottom, self.from_side)
        };
        match (
            first_distance > f32::EPSILON,
            second_distance > f32::EPSILON,
        ) {
            (true, true) => 0.5,
            (true, false) => 1.0,
            (false, true) => 0.0,
            (false, false) => 0.5,
        }
    }

    fn phase_progress(progress: f32, start: f32, end: f32) -> f32 {
        if end <= start {
            return 1.0;
        }
        ((progress - start) / (end - start)).clamp(0.0, 1.0)
    }

    pub(crate) fn spans_layout(&self, now: Instant) -> bool {
        let progress = self.progress(now);
        if progress >= 1.0 {
            return self.target_open;
        }

        let split = self.phase_split();
        if self.target_open {
            self.from_bottom > f32::EPSILON || progress >= split
        } else {
            self.from_bottom > f32::EPSILON && progress < split
        }
    }

    pub(crate) fn bottom_extent(&self, now: Instant) -> f32 {
        let progress = self.progress(now);
        let split = self.phase_split();
        if self.target_open {
            let phase = Self::ease(Self::phase_progress(progress, split, 1.0));
            self.from_bottom + (1.0 - self.from_bottom) * phase
        } else {
            let phase = Self::ease(Self::phase_progress(progress, 0.0, split));
            self.from_bottom * (1.0 - phase)
        }
    }

    pub(crate) fn side_shortening(&self, now: Instant) -> f32 {
        let progress = self.progress(now);
        let split = self.phase_split();
        if self.target_open {
            let phase = Self::ease(Self::phase_progress(progress, 0.0, split));
            self.from_side + (1.0 - self.from_side) * phase
        } else {
            let phase = Self::ease(Self::phase_progress(progress, split, 1.0));
            self.from_side * (1.0 - phase)
        }
    }

    pub(crate) fn is_active(&self, now: Instant) -> bool {
        self.progress(now) < 1.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HeaderMenuSection {
    File,
    Edit,
    View,
    Window,
    Help,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResizeMode {
    PanelWidth,
    BottomSpan,
    BottomHeight,
    RestoreLeft,
    RestoreRight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResizeTarget {
    Left,
    Right,
    Bottom,
}

#[derive(Clone)]
pub(crate) struct ShellState {
    pub focused_panel: PanelPosition,
    pub main_panel: PanelHost,
    pub left_panel: PanelHost,
    pub bottom_panel: PanelHost,
    pub right_panel: PanelHost,
    pub bottom_span_left: bool,
    pub bottom_span_right: bool,
    pub bottom_span_left_transition: Option<PanelSpanTransition>,
    pub bottom_span_right_transition: Option<PanelSpanTransition>,
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
    pub resize_start_pos: Option<gpui::Point<gpui::Pixels>>,
    pub resize_mode: Option<ResizeMode>,
}

impl Default for ShellState {
    fn default() -> Self {
        Self {
            focused_panel: PanelPosition::Main,
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
            bottom_span_left: false,
            bottom_span_right: false,
            bottom_span_left_transition: None,
            bottom_span_right_transition: None,
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
            resize_start_pos: None,
            resize_mode: None,
        }
    }
}

impl ShellState {
    pub(crate) fn bottom_spans_left_layout(&self, now: Instant) -> bool {
        self.bottom_span_left_transition
            .as_ref()
            .map(|transition| transition.spans_layout(now))
            .unwrap_or(self.bottom_span_left)
    }

    pub(crate) fn bottom_spans_right_layout(&self, now: Instant) -> bool {
        self.bottom_span_right_transition
            .as_ref()
            .map(|transition| transition.spans_layout(now))
            .unwrap_or(self.bottom_span_right)
    }

    pub(crate) fn bottom_left_extent(&self, now: Instant) -> f32 {
        self.bottom_span_left_transition
            .as_ref()
            .map(|transition| transition.bottom_extent(now))
            .unwrap_or(if self.bottom_span_left { 1.0 } else { 0.0 })
    }

    pub(crate) fn bottom_right_extent(&self, now: Instant) -> f32 {
        self.bottom_span_right_transition
            .as_ref()
            .map(|transition| transition.bottom_extent(now))
            .unwrap_or(if self.bottom_span_right { 1.0 } else { 0.0 })
    }

    pub(crate) fn left_side_shortening(&self, now: Instant) -> f32 {
        self.bottom_span_left_transition
            .as_ref()
            .map(|transition| transition.side_shortening(now))
            .unwrap_or(if self.bottom_span_left { 1.0 } else { 0.0 })
    }

    pub(crate) fn right_side_shortening(&self, now: Instant) -> f32 {
        self.bottom_span_right_transition
            .as_ref()
            .map(|transition| transition.side_shortening(now))
            .unwrap_or(if self.bottom_span_right { 1.0 } else { 0.0 })
    }

    pub(crate) fn bottom_span_is_animating(&self, now: Instant) -> bool {
        self.bottom_span_left_transition
            .as_ref()
            .is_some_and(|transition| transition.is_active(now))
            || self
                .bottom_span_right_transition
                .as_ref()
                .is_some_and(|transition| transition.is_active(now))
    }
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
    fn left_and_right_span_transitions_are_independent() {
        let start = Instant::now();
        let left_state = ShellState {
            bottom_span_left: true,
            bottom_span_left_transition: Some(PanelSpanTransition::from_visual(
                0.0, 0.0, true, start,
            )),
            ..ShellState::default()
        };

        assert!(left_state.left_side_shortening(start + Duration::from_millis(45)) > 0.0);
        assert_eq!(left_state.right_side_shortening(start), 0.0);

        let both_state = ShellState {
            bottom_span_left: true,
            bottom_span_right: true,
            bottom_span_left_transition: Some(PanelSpanTransition::from_visual(
                0.0, 0.0, true, start,
            )),
            bottom_span_right_transition: Some(PanelSpanTransition::from_visual(
                0.0, 0.0, true, start,
            )),
            ..ShellState::default()
        };

        assert!(both_state.right_side_shortening(start + Duration::from_millis(45)) > 0.0);
        assert!(both_state.bottom_span_is_animating(start));
    }
    #[test]
    fn span_transition_runs_side_and_bottom_in_sequence() {
        let start = Instant::now();
        let opening = PanelSpanTransition::from_visual(0.0, 0.0, true, start);

        let opening_side_phase = start + Duration::from_millis(40);
        assert!(!opening.spans_layout(opening_side_phase));
        assert!(opening.side_shortening(opening_side_phase) > 0.0);
        assert_eq!(opening.bottom_extent(opening_side_phase), 0.0);

        let opening_bottom_phase = start + Duration::from_millis(120);
        assert!(opening.spans_layout(opening_bottom_phase));
        assert_eq!(opening.side_shortening(opening_bottom_phase), 1.0);
        assert!(opening.bottom_extent(opening_bottom_phase) > 0.0);

        let closing = PanelSpanTransition::from_visual(1.0, 1.0, false, start);
        let closing_bottom_phase = start + Duration::from_millis(40);
        assert!(closing.spans_layout(closing_bottom_phase));
        assert_eq!(closing.side_shortening(closing_bottom_phase), 1.0);
        assert!(closing.bottom_extent(closing_bottom_phase) < 1.0);

        let closing_side_phase = start + Duration::from_millis(120);
        assert!(!closing.spans_layout(closing_side_phase));
        assert_eq!(closing.bottom_extent(closing_side_phase), 0.0);
        assert!(closing.side_shortening(closing_side_phase) < 1.0);
    }

    #[test]
    fn span_transition_reverses_from_the_current_visual_state() {
        let start = Instant::now();
        let opening = PanelSpanTransition::from_visual(0.0, 0.0, true, start);
        let reverse_at = start + Duration::from_millis(120);
        let side_at_reverse = opening.side_shortening(reverse_at);
        let bottom_at_reverse = opening.bottom_extent(reverse_at);
        let closing =
            PanelSpanTransition::from_visual(side_at_reverse, bottom_at_reverse, false, reverse_at);

        let after_reverse = reverse_at + Duration::from_millis(40);
        assert_eq!(closing.side_shortening(after_reverse), side_at_reverse);
        assert!(closing.bottom_extent(after_reverse) < bottom_at_reverse);

        let reopen_at = after_reverse;
        let reopening = PanelSpanTransition::from_visual(
            closing.side_shortening(reopen_at),
            closing.bottom_extent(reopen_at),
            true,
            reopen_at,
        );
        assert!(
            reopening.bottom_extent(reopen_at + Duration::from_millis(40))
                > closing.bottom_extent(reopen_at)
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
