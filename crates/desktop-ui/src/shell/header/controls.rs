use super::*;

#[derive(Clone, Copy)]
pub(crate) enum WindowControlIcon {
    Minimize,
    Maximize,
    Close,
}

impl WindowControlIcon {
    fn icon_name(self) -> crate::components::IconName {
        match self {
            Self::Minimize => crate::components::IconName::Minus,
            Self::Maximize => crate::components::IconName::Square,
            Self::Close => crate::components::IconName::X,
        }
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
    icon: WindowControlIcon,
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
        .child(crate::components::Icon::new(icon.icon_name()))
}
