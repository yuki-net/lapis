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
            .text_color(theme::muted())
            .child(crate::components::Icon::new(name).with_rotation(rotation))
    }
}

/// A visible Lucide menu icon for the header.
pub(crate) fn menu_icon() -> gpui::Div {
    div()
        .size_4()
        .flex()
        .items_center()
        .justify_center()
        .text_color(theme::muted())
        .child(crate::components::Icon::new(
            crate::components::IconName::Menu,
        ))
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
            .text_color(theme::muted())
            .child(crate::components::Icon::new(self.name.icon_name()))
    }
}

/// タイトルバーのツールボタン。アクティブ状態をアクセントで表す。
pub(crate) fn top_icon(label: impl Into<SharedString>, active: bool) -> gpui::Stateful<gpui::Div> {
    let label = label.into();
    div()
        .id(label.clone())
        .size(px(28.0))
        .rounded(px(6.0))
        .flex()
        .items_center()
        .justify_center()
        .occlude()
        .bg(if active {
            theme::accent_soft()
        } else {
            theme::title_bar()
        })
        .text_color(if active {
            rgb(0xbfc0ff)
        } else {
            theme::muted()
        })
        .text_size(px(14.0))
        .hover(|style| style.bg(theme::surface_hover()).text_color(theme::text()))
        .child(label)
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
        .w(px(theme::WINDOW_CONTROL_WIDTH))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .occlude()
        .window_control_area(area)
        .text_color(theme::muted())
        .hover(move |style| {
            if close {
                style.bg(theme::close_hover()).text_color(rgb(0xffffff))
            } else {
                style.bg(theme::surface_hover()).text_color(theme::text())
            }
        })
        .active(|style| style.bg(theme::surface_active()))
        .child(WindowControlIcon::new(icon))
}
