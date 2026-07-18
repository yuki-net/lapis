use gpui::{SharedString, div, prelude::*, px};

use crate::theme;

pub(crate) fn tool_empty_state(
    icon: &'static str,
    title: &'static str,
    message: &'static str,
    detail: &'static str,
) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .items_center()
        .justify_center()
        .px_3()
        .gap_2()
        .text_center()
        .child(
            div()
                .size(px(34.0))
                .rounded(px(8.0))
                .flex()
                .items_center()
                .justify_center()
                .bg(theme::surface())
                .text_size(px(17.0))
                .text_color(theme::muted())
                .child(icon),
        )
        .child(
            div()
                .text_size(px(12.0))
                .text_color(theme::text())
                .child(title),
        )
        .child(
            div()
                .text_size(px(11.0))
                .text_color(theme::muted())
                .child(message),
        )
        .child(
            div()
                .text_size(px(10.0))
                .text_color(theme::subtle())
                .child(detail),
        )
}

pub(crate) fn panel_empty_state(
    icon: impl Into<SharedString>,
    message: impl Into<SharedString>,
    detail: impl Into<SharedString>,
) -> gpui::Div {
    let icon = icon.into();
    let message = message.into();
    let detail = detail.into();
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .px_4()
        .text_center()
        .child(
            div()
                .text_size(px(18.0))
                .text_color(theme::subtle())
                .child(icon),
        )
        .child(
            div()
                .text_size(px(11.0))
                .text_color(theme::muted())
                .child(message),
        )
        .child(
            div()
                .text_size(px(10.0))
                .text_color(theme::subtle())
                .child(detail),
        )
}
