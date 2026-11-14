use std::f32::consts::{FRAC_PI_2, PI};

use super::*;

#[derive(Clone, Copy)]
pub(crate) enum ZonePosition {
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
#[derive(IntoElement)]
pub(crate) struct PanelToggleIcon {
    position: ZonePosition,
    state: PanelState,
}

impl PanelToggleIcon {
    pub(crate) const fn new(position: ZonePosition, state: PanelState) -> Self {
        Self { position, state }
    }
}

impl gpui::RenderOnce for PanelToggleIcon {
    fn render(self, _: &mut Window, cx: &mut gpui::App) -> impl IntoElement {
        let name = match self.state {
            PanelState::Open => crate::components::IconName::PanelLeft,
            PanelState::Close => crate::components::IconName::PanelLeftDashed,
        };
        let rotation = match self.position {
            ZonePosition::Left => 0.0,
            ZonePosition::Bottom => FRAC_PI_2,
            ZonePosition::Right => PI,
        };

        div()
            .size_4()
            .flex()
            .items_center()
            .justify_center()
            .text_color(theme::colors(cx).text_secondary)
            .child(crate::components::Icon::new(name).with_rotation(rotation))
    }
}

pub(crate) fn open_panel_icon(position: extension_ui_contract::ZonePosition) -> PanelToggleIcon {
    let position = match position {
        extension_ui_contract::ZonePosition::Left => ZonePosition::Left,
        extension_ui_contract::ZonePosition::Bottom => ZonePosition::Bottom,
        extension_ui_contract::ZonePosition::Right => ZonePosition::Right,
        extension_ui_contract::ZonePosition::Main => ZonePosition::Left,
    };
    PanelToggleIcon::new(position, PanelState::Open)
}
