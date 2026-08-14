use std::time::{Duration, Instant};

use crate::extension_ui::{PanelPosition, ViewId};

use super::{
    panel_transition::{PANEL_ANIMATION_DURATION, PanelTransition},
    tab_state::PanelTab,
};

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
    pub(super) fn new(position: PanelPosition, tabs: Vec<PanelTab>, open: bool, size: f32) -> Self {
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
