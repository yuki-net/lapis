use gpui::{div, prelude::*, px};

use crate::theme;

pub(crate) fn tool_tab(index: usize, label: String, active: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(("tool-tab", index))
        .h(px(31.0))
        .px(px(8.0))
        .rounded_t(px(6.0))
        .flex()
        .items_center()
        .bg(if active {
            theme::surface()
        } else {
            theme::island()
        })
        .text_color(if active {
            theme::text()
        } else {
            theme::muted()
        })
        .text_size(px(12.0))
        .hover(|style| style.bg(theme::surface_hover()).text_color(theme::text()))
        .child(label)
}

pub(crate) fn panel_tab(index: usize, label: String, active: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(("bottom-panel-tab", index))
        .h(px(28.0))
        .px_2()
        .rounded(px(5.0))
        .flex()
        .items_center()
        .bg(if active {
            theme::surface()
        } else {
            theme::island()
        })
        .text_size(px(11.0))
        .text_color(if active {
            theme::text()
        } else {
            theme::muted()
        })
        .child(label)
}
