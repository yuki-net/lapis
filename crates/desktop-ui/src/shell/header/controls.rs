use super::*;

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
    label: impl Into<SharedString>,
    area: WindowControlArea,
    close: bool,
) -> gpui::Stateful<gpui::Div> {
    let label = label.into();
    div()
        .id(id)
        .h(px(
            theme::TITLE_BAR_HEIGHT - theme::WINDOW_RESIZE_BORDER_HEIGHT
        ))
        .w(px(theme::WINDOW_CONTROL_WIDTH))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .window_control_area(area)
        .text_size(px(14.0))
        .text_color(theme::muted())
        .hover(move |style| {
            if close {
                style.bg(theme::close_hover()).text_color(rgb(0xffffff))
            } else {
                style.bg(theme::surface_hover()).text_color(theme::text())
            }
        })
        .active(|style| style.bg(theme::surface_active()))
        .child(label)
}
