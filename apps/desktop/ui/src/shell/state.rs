use std::time::{Duration, Instant};

use lapis_desktop_client::DocumentTab;

use crate::extension_ui::{FeatureRegistry, ZonePosition};

use super::{DockLayout, PanelHost, PanelId, PanelTab, ViewInstanceId};

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PickerTarget {
    Panel(PanelId),
    Zone(ZonePosition),
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

#[derive(Clone, Debug)]
pub(super) struct PanelResize {
    pub(super) boundary: super::dock_geometry::PanelBoundary,
    pub(super) previous: gpui::Point<gpui::Pixels>,
}

#[derive(Clone)]
pub(crate) struct ShellState {
    pub focused_panel: Option<PanelId>,
    pub layout: DockLayout,
    pub bottom_span_left_transition: Option<PanelSpanTransition>,
    pub bottom_span_right_transition: Option<PanelSpanTransition>,
    pub command_palette_open: bool,
    pub show_panel_controls: bool,
    pub tool_picker: Option<PickerTarget>,
    pub tool_picker_anchor: gpui::Point<gpui::Pixels>,
    pub tool_picker_query: String,
    pub tool_picker_selected: usize,
    pub settings_menu_open: bool,
    pub settings_menu_anchor: gpui::Point<gpui::Pixels>,
    pub header_menu_open: bool,
    pub header_menu_anchor: gpui::Point<gpui::Pixels>,
    pub header_menu_section: Option<HeaderMenuSection>,
    pub theme_picker_open: bool,
    pub theme_save_in_flight: bool,
    pub resizing: Option<ResizeTarget>,
    pub resize_start_pos: Option<gpui::Point<gpui::Pixels>>,
    pub resize_mode: Option<ResizeMode>,
    pub(super) panel_resize: Option<PanelResize>,
}

impl Default for ShellState {
    fn default() -> Self {
        Self {
            focused_panel: None,
            layout: DockLayout::default(),
            bottom_span_left_transition: None,
            bottom_span_right_transition: None,
            command_palette_open: false,
            show_panel_controls: true,
            tool_picker: None,
            tool_picker_anchor: gpui::point(gpui::px(120.0), gpui::px(82.0)),
            tool_picker_query: String::new(),
            tool_picker_selected: 0,
            settings_menu_open: false,
            settings_menu_anchor: gpui::point(gpui::px(0.0), gpui::px(0.0)),
            header_menu_open: false,
            header_menu_anchor: gpui::point(gpui::px(0.0), gpui::px(0.0)),
            header_menu_section: None,
            theme_picker_open: false,
            theme_save_in_flight: false,
            resizing: None,
            resize_start_pos: None,
            resize_mode: None,
            panel_resize: None,
        }
    }
}

impl ShellState {
    /// 消えた追加先へ操作を適用しない。共有Viewは個体を保って移動する。
    pub(crate) fn open_panel_tab(&mut self, group: &PanelId, tab: PanelTab) -> bool {
        if self.layout.panel(group).is_none() {
            return false;
        }
        if let Some(source) = self.layout.panel_for_tab(&tab) {
            if self
                .layout
                .drop_tab(
                    &source,
                    &tab,
                    &super::PanelDrop::Into(group.clone()),
                    (0.0, 0.0),
                    Instant::now(),
                )
                .is_err()
            {
                return false;
            }
        } else if !self.layout.insert_tab_in_panel(group, tab, false) {
            return false;
        }
        self.layout.set_panel_open(group, true);
        self.focused_panel = Some(group.clone());
        true
    }

    pub(crate) fn bottom_spans_left_layout(&self, now: Instant) -> bool {
        self.bottom_span_left_transition
            .as_ref()
            .map(|transition| transition.spans_layout(now))
            .unwrap_or(self.layout.bottom_span_left)
    }

    pub(crate) fn bottom_spans_right_layout(&self, now: Instant) -> bool {
        self.bottom_span_right_transition
            .as_ref()
            .map(|transition| transition.spans_layout(now))
            .unwrap_or(self.layout.bottom_span_right)
    }

    pub(crate) fn bottom_left_extent(&self, now: Instant) -> f32 {
        self.bottom_span_left_transition
            .as_ref()
            .map(|transition| transition.bottom_extent(now))
            .unwrap_or(if self.layout.bottom_span_left {
                1.0
            } else {
                0.0
            })
    }

    pub(crate) fn bottom_right_extent(&self, now: Instant) -> f32 {
        self.bottom_span_right_transition
            .as_ref()
            .map(|transition| transition.bottom_extent(now))
            .unwrap_or(if self.layout.bottom_span_right {
                1.0
            } else {
                0.0
            })
    }

    pub(crate) fn left_side_shortening(&self, now: Instant) -> f32 {
        self.bottom_span_left_transition
            .as_ref()
            .map(|transition| transition.side_shortening(now))
            .unwrap_or(if self.layout.bottom_span_left {
                1.0
            } else {
                0.0
            })
    }

    pub(crate) fn right_side_shortening(&self, now: Instant) -> f32 {
        self.bottom_span_right_transition
            .as_ref()
            .map(|transition| transition.side_shortening(now))
            .unwrap_or(if self.layout.bottom_span_right {
                1.0
            } else {
                0.0
            })
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
    pub fn panels(&self) -> Vec<&PanelHost> {
        self.layout.panels()
    }

    pub fn set_tool_picker_query(&mut self, query: impl Into<String>) {
        self.tool_picker_query = query.into();
        self.tool_picker_selected = 0;
    }

    pub(crate) fn next_tool_instance(
        &mut self,
        view: &crate::extension_ui::ViewId,
    ) -> Result<ViewInstanceId, getrandom::Error> {
        // 復元・移動・非表示の経路に依存せず、Layout全体の個体IDと照合する。
        loop {
            // Window・非表示Task・再起動をまたぐため、Layout内の連番に依存しない。
            let mut bytes = [0u8; 16];
            getrandom::fill(&mut bytes)?;
            let token = bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            let instance = ViewInstanceId::new(format!("{}:{token}", view.as_str()));
            if !self
                .panels()
                .iter()
                .any(|panel| panel.tabs.iter().any(|tab| tab.instance_id() == instance))
            {
                return Ok(instance);
            }
        }
    }

    pub fn synchronize_documents(&mut self, documents: &[DocumentTab]) {
        let valid = documents
            .iter()
            .map(|document| &document.id)
            .collect::<Vec<_>>();
        let stale = self
            .layout
            .placements()
            .into_iter()
            .filter(|(tab, _)| matches!(tab, PanelTab::Document(id) if !valid.contains(&id)))
            .collect::<Vec<_>>();
        for (tab, group) in stale {
            self.layout.remove_tab(&group, &tab, Instant::now());
        }
        for document in documents {
            let tab = PanelTab::Document(document.id.clone());
            if !self.panels().iter().any(|panel| panel.contains(&tab)) {
                let _ = self.layout.open_tab(ZonePosition::Main, tab.clone());
            }
            if document.active {
                self.layout.activate_existing_tab(&tab);
            }
        }
    }

    pub fn synchronize_activation(&self, registry: &mut FeatureRegistry) {
        registry.set_visible_views(
            self.layout
                .visible_panels()
                .filter_map(|panel| panel.active.as_ref().and_then(PanelTab::view_id).cloned()),
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::{extension_ui::ViewId, features::id};
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn picker_adds_to_clicked_group_and_preserves_shared_identity() {
        let mut shell = ShellState::default();
        let left = shell
            .layout
            .first_panel_in_zone(ZonePosition::Left)
            .unwrap();
        let tab = PanelTab::tool(id::VIEW_FILES);
        let other = PanelTab::tool("other");
        shell
            .layout
            .open_tab(ZonePosition::Left, other.clone())
            .unwrap();
        let second = shell
            .layout
            .drop_tab(
                &left,
                &other,
                &crate::shell::PanelDrop::Edge {
                    panel: left.clone(),
                    edge: crate::shell::PanelEdge::Right,
                },
                (600.0, 400.0),
                Instant::now(),
            )
            .unwrap();
        assert!(shell.open_panel_tab(&second, tab.clone()));
        assert!(!shell.layout.panel(&left).unwrap().contains(&tab));
        assert!(shell.layout.panel(&second).unwrap().contains(&tab));
        assert_eq!(shell.focused_panel, Some(second));
        let before = shell.layout.capture();
        assert!(!shell.open_panel_tab(&PanelId::new(999), tab));
        assert_eq!(shell.layout.capture(), before);
        assert!(shell.layout.valid());
    }

    #[test]
    fn initializes_four_zones_with_one_permanent_main_panel() {
        let state = ShellState::default();
        assert_eq!(state.layout.capture().zones.len(), 4);
        assert!(state.layout.zone_open(ZonePosition::Main));
        assert!(state.layout.zone_open(ZonePosition::Left));
        assert!(!state.layout.zone_open(ZonePosition::Bottom));
        assert!(!state.layout.zone_open(ZonePosition::Right));
        assert_eq!(state.panels().len(), 2);
        let main = state
            .layout
            .first_panel_in_zone(ZonePosition::Main)
            .unwrap();
        assert!(state.layout.panel(&main).unwrap().tabs.is_empty());
        assert!(
            state
                .layout
                .find_tab(&PanelTab::tool(id::VIEW_FILES))
                .is_some()
        );
    }

    #[test]
    fn generated_tool_instances_are_stable_and_distinct() {
        let mut state = ShellState::default();
        let view = ViewId::new(id::VIEW_TERMINAL);

        let first = state.next_tool_instance(&view).unwrap();
        let second = state.next_tool_instance(&view).unwrap();

        assert_ne!(first, second);
        assert!(first.as_str().starts_with("lapis.view.terminal:"));
        assert!(second.as_str().starts_with("lapis.view.terminal:"));
    }

    #[test]
    fn restored_hidden_instances_do_not_collide_with_new_tabs() {
        let view = ViewId::new(id::VIEW_TERMINAL);
        let mut original = ShellState::default();
        for index in [0, 1, 3] {
            original
                .layout
                .open_tab(
                    ZonePosition::Bottom,
                    PanelTab::tool_instance(view.clone(), format!("{}:{index}", view.as_str())),
                )
                .unwrap();
        }
        original.layout.set_zone_open(ZonePosition::Bottom, false);
        let saved = original.layout.capture();
        let mut restored = ShellState {
            layout: DockLayout::restore(&saved, |raw| {
                let (view, instance) = raw.strip_prefix("tool:")?.split_once(':')?;
                Some(PanelTab::tool_instance(view, instance))
            })
            .unwrap(),
            ..Default::default()
        };
        for _ in 0..2 {
            let instance = restored.next_tool_instance(&view).unwrap();
            assert!(
                !original
                    .panels()
                    .iter()
                    .any(|panel| panel.tabs.iter().any(|tab| tab.instance_id() == instance))
            );
            restored
                .layout
                .open_tab(
                    ZonePosition::Main,
                    PanelTab::tool_instance(view.clone(), instance),
                )
                .unwrap();
        }
        assert!(restored.layout.valid());
        assert!(!restored.layout.zone_open(ZonePosition::Bottom));
    }

    #[test]
    fn editing_picker_query_resets_keyboard_selection() {
        let mut shell = ShellState {
            tool_picker_selected: 10,
            ..Default::default()
        };
        shell.set_tool_picker_query("terminal");
        assert_eq!(shell.tool_picker_selected, 0);
        shell.tool_picker_selected = 4;
        shell.set_tool_picker_query("termina");
        assert_eq!(shell.tool_picker_selected, 0);
    }

    #[test]
    fn left_and_right_span_transitions_are_independent() {
        let start = Instant::now();
        let left_state = ShellState {
            bottom_span_left_transition: Some(PanelSpanTransition::from_visual(
                0.0, 0.0, true, start,
            )),
            ..ShellState::default()
        };

        assert!(left_state.left_side_shortening(start + Duration::from_millis(45)) > 0.0);
        assert_eq!(left_state.right_side_shortening(start), 0.0);

        let both_state = ShellState {
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
        let source = state.layout.find_tab(&tab).unwrap();
        let target = state
            .layout
            .move_tab_to_zone(ZonePosition::Main, tab.clone(), Instant::now())
            .unwrap();
        assert_ne!(source, target);
        assert_eq!(state.layout.panel(&target).unwrap().active, Some(tab));
        assert!(state.layout.panel(&source).unwrap().tabs.is_empty());
    }

    #[test]
    fn moving_a_tab_to_closed_panel_does_not_change_state_at_action_boundary() {
        let mut state = ShellState::default();
        let tab = PanelTab::tool(id::VIEW_FILES);
        let source = state.layout.find_tab(&tab).unwrap();
        let target = state
            .layout
            .open_tab(ZonePosition::Right, PanelTab::tool("other"))
            .unwrap();
        state.layout.set_panel_open(&target, false);
        let before = state.layout.capture();
        assert!(
            state
                .layout
                .drop_tab(
                    &source,
                    &tab,
                    &crate::shell::PanelDrop::Into(target),
                    (400.0, 400.0),
                    Instant::now()
                )
                .is_err()
        );
        assert_eq!(state.layout.capture(), before);
    }

    #[test]
    fn removing_last_tab_closes_panel_after_transition() {
        let mut state = ShellState::default();
        let files = PanelTab::tool(id::VIEW_FILES);
        let group = state.layout.find_tab(&files).unwrap();
        let terminal = PanelTab::tool(id::VIEW_TERMINAL);
        state
            .layout
            .open_tab(ZonePosition::Left, terminal.clone())
            .unwrap();
        assert!(state.layout.remove_tab(&group, &files, Instant::now()));
        assert_eq!(
            state.layout.panel(&group).unwrap().active,
            Some(terminal.clone())
        );
        let now = Instant::now();
        assert!(state.layout.remove_tab(&group, &terminal, now));
        assert!(state.layout.panel(&group).is_some());
        state
            .layout
            .finish_due_closes(now + crate::shell::panel_transition::PANEL_ANIMATION_DURATION);
        assert!(state.layout.panel(&group).is_none());
        assert!(!state.layout.zone_open(ZonePosition::Left));
    }

    #[test]
    fn closing_panel_remains_until_its_transition_completes() {
        let mut state = ShellState::default();
        let tab = PanelTab::tool(id::VIEW_FILES);
        let group = state.layout.find_tab(&tab).unwrap();
        let now = Instant::now();
        assert!(state.layout.remove_tab(&group, &tab, now));
        assert!(!state.layout.finish_due_closes(now));
        assert!(state.layout.panel(&group).is_some());
        assert!(
            state
                .layout
                .finish_due_closes(now + crate::shell::panel_transition::PANEL_ANIMATION_DURATION)
        );
        assert!(state.layout.panel(&group).is_none());
    }

    #[test]
    fn reopening_a_panel_cancels_its_pending_empty_close() {
        let mut state = ShellState::default();
        let tab = PanelTab::tool(id::VIEW_FILES);
        let group = state.layout.find_tab(&tab).unwrap();
        let now = Instant::now();
        state.layout.remove_tab(&group, &tab, now);
        assert!(state.open_panel_tab(&group, tab.clone()));
        state
            .layout
            .finish_due_closes(now + crate::shell::panel_transition::PANEL_ANIMATION_DURATION);
        assert!(state.layout.panel(&group).unwrap().contains(&tab));
    }

    #[test]
    fn restored_layout_excludes_empty_closing_panels() {
        let mut state = ShellState::default();
        let tab = PanelTab::tool(id::VIEW_FILES);
        let group = state.layout.find_tab(&tab).unwrap();
        state.layout.remove_tab(&group, &tab, Instant::now());
        let saved = state.layout.capture();
        let restored = DockLayout::restore(&saved, |_| None).unwrap();
        assert!(restored.panel(&group).is_none());
        assert!(!restored.has_closing_panels());
    }
}
