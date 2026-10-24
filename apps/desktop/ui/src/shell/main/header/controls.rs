use std::f32::consts::{FRAC_PI_2, PI};

use super::*;

#[derive(Clone, Copy)]
pub(crate) enum PanelPosition {
    Left,
    Bottom,
    Right,
}

#[derive(Clone, Copy)]
pub(crate) enum PanelState {
    Open,
    Close,
}

/// Reuses the Lucide PanelLeft pair for every dock by rotating the SVG.
pub(crate) struct PanelToggleIcon {
    position: PanelPosition,
    state: PanelState,
}

impl PanelToggleIcon {
    pub(crate) const fn new(position: PanelPosition, state: PanelState) -> Self {
        Self { position, state }
    }
}

impl IntoElement for PanelToggleIcon {
    type Element = gpui::Div;

    fn into_element(self) -> Self::Element {
        let name = match self.state {
            PanelState::Open => crate::components::IconName::PanelLeft,
            PanelState::Close => crate::components::IconName::PanelLeftDashed,
        };
        let rotation = match self.position {
            PanelPosition::Left => 0.0,
            PanelPosition::Bottom => FRAC_PI_2,
            PanelPosition::Right => PI,
        };

        div()
            .size_4()
            .flex()
            .items_center()
            .justify_center()
            .text_color(theme::colors().text_secondary)
            .child(crate::components::Icon::new(name).with_rotation(rotation))
    }
}

pub(crate) fn open_panel_icon(position: crate::extension_ui::PanelPosition) -> PanelToggleIcon {
    let position = match position {
        crate::extension_ui::PanelPosition::Left => PanelPosition::Left,
        crate::extension_ui::PanelPosition::Bottom => PanelPosition::Bottom,
        crate::extension_ui::PanelPosition::Right => PanelPosition::Right,
        crate::extension_ui::PanelPosition::Main => PanelPosition::Left,
    };
    PanelToggleIcon::new(position, PanelState::Open)
}
