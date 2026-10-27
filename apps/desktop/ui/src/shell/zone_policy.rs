//! Zoneごとの配置と操作の方針。状態とResourceの寿命は保持しない。

use super::panel_layout::{Axis, PanelEdge};
use crate::extension_ui::ZonePosition;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ZoneSizing {
    FillRemaining,
    Proportional,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ZonePolicy {
    pub required: bool,
    pub user_closable: bool,
    pub min_panels: usize,
    pub max_panels: Option<usize>,
    pub keep_empty_panel: bool,
    pub growth_edge: Option<PanelEdge>,
    pub sizing: ZoneSizing,
    pub preferred_extent: f32,
    pub interactive_min_extent: f32,
    pub constrained_min_extent: f32,
    pub close_threshold: f32,
}

pub(super) struct PanelConstraints {
    pub interactive_min_extent: f32,
    pub constrained_min_extent: f32,
    pub close_threshold: f32,
}

impl PanelConstraints {
    pub const COMMON: Self = Self {
        interactive_min_extent: 32.0,
        constrained_min_extent: 0.0,
        close_threshold: 16.0,
    };
}

impl ZonePolicy {
    pub fn for_position(position: ZonePosition) -> Self {
        let (required, growth_edge, preferred_extent) = match position {
            ZonePosition::Main => (true, None, 0.0),
            ZonePosition::Left => (
                false,
                Some(PanelEdge::Right),
                f32::from(crate::tokens::size::TOOL_ISLAND_WIDTH),
            ),
            ZonePosition::Right => (
                false,
                Some(PanelEdge::Left),
                f32::from(crate::tokens::size::SIDE_PANEL_WIDTH),
            ),
            ZonePosition::Bottom => (
                false,
                Some(PanelEdge::Top),
                f32::from(crate::tokens::size::BOTTOM_PANEL_HEIGHT),
            ),
        };
        Self {
            required,
            user_closable: !required,
            min_panels: usize::from(required),
            max_panels: Some(if required { 1 } else { 3 }),
            keep_empty_panel: required,
            growth_edge,
            sizing: if required {
                ZoneSizing::FillRemaining
            } else {
                ZoneSizing::Proportional
            },
            preferred_extent,
            interactive_min_extent: 32.0,
            constrained_min_extent: 0.0,
            close_threshold: 16.0,
        }
    }

    pub fn accepts_count(self, count: usize) -> bool {
        count >= self.min_panels && self.max_panels.is_none_or(|max| count <= max)
    }

    pub fn default_extent_ratio(self) -> f32 {
        match self.growth_edge {
            None => 1.0,
            Some(PanelEdge::Left | PanelEdge::Right) => {
                self.preferred_extent / f32::from(crate::tokens::size::DEFAULT_WINDOW_WIDTH)
            }
            Some(PanelEdge::Top) => {
                self.preferred_extent / f32::from(crate::tokens::size::DEFAULT_WINDOW_HEIGHT)
            }
        }
    }

    pub fn can_add(self, count: usize) -> bool {
        self.growth_edge.is_some() && self.max_panels.is_none_or(|max| count < max)
    }

    pub fn growth(self) -> Option<(Axis, bool)> {
        match self.growth_edge? {
            PanelEdge::Left => Some((Axis::Horizontal, true)),
            PanelEdge::Right => Some((Axis::Horizontal, false)),
            PanelEdge::Top => Some((Axis::Vertical, true)),
        }
    }
}
