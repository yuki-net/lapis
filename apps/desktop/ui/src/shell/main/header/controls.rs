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
#[derive(Clone, Copy)]
pub(crate) enum WindowControlIconName {
    Minimize,
    Maximize,
    Close,
}

impl WindowControlIconName {
    fn icon_name(self) -> crate::components::IconName {
        match self {
            Self::Minimize => crate::components::IconName::Minus,
            Self::Maximize => crate::components::IconName::Square,
            Self::Close => crate::components::IconName::X,
        }
    }
}

/// ネイティブのウィンドウ操作ボタン用 Lucide アイコン。
///
/// 色は共通の `Icon` コンポーネントが有効テーマから解決する。
pub(crate) struct WindowControlIcon {
    name: WindowControlIconName,
}

impl WindowControlIcon {
    pub(crate) const fn new(name: WindowControlIconName) -> Self {
        Self { name }
    }
}

impl IntoElement for WindowControlIcon {
    type Element = gpui::Div;

    fn into_element(self) -> Self::Element {
        div()
            .size_4()
            .flex()
            .items_center()
            .justify_center()
            .text_color(theme::colors().text_secondary)
            .child(crate::components::Icon::new(self.name.icon_name()))
    }
}

/// ウィンドウコントロールボタン（最小化・最大化・閉じる）。
pub(crate) fn window_control_button(
    id: &'static str,
    icon: WindowControlIconName,
    area: WindowControlArea,
    close: bool,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h_full()
        .w(tokens::size::WINDOW_CONTROL_WIDTH)
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .occlude()
        .window_control_area(area)
        .text_color(theme::colors().text_secondary)
        .hover(move |style| {
            if close {
                style
                    .bg(theme::colors().danger_background)
                    .text_color(theme::colors().text_primary)
            } else {
                style
                    .bg(theme::colors().button_background_hover)
                    .text_color(theme::colors().text_primary)
            }
        })
        .active(|style| style.bg(theme::colors().button_background_selected))
        .child(WindowControlIcon::new(icon))
}
