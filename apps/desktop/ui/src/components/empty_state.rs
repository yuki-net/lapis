use gpui::{SharedString, div, prelude::*};

use crate::{theme, tokens};

pub(crate) fn panel_empty_state(
    icon: impl Into<SharedString>,
    message: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    cx: &gpui::App,
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
        .gap(tokens::spacing::XS)
        .px(tokens::spacing::LG)
        .text_center()
        .child(
            div()
                .text_size(tokens::typography::FONT_XL)
                .text_color(theme::colors(cx).text_tertiary)
                .child(icon),
        )
        .child(
            div()
                .text_size(tokens::typography::FONT_XS)
                .text_color(theme::colors(cx).text_secondary)
                .child(message),
        )
        .child(
            div()
                .text_size(tokens::typography::FONT_XS)
                .text_color(theme::colors(cx).text_tertiary)
                .child(detail),
        )
}

pub(crate) fn panel_empty_state_element(
    icon: impl IntoElement,
    message: impl Into<SharedString>,
    detail: impl Into<SharedString>,
    cx: &gpui::App,
) -> gpui::Div {
    let message = message.into();
    let detail = detail.into();
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(tokens::spacing::XS)
        .px(tokens::spacing::LG)
        .text_center()
        .child(icon)
        .child(
            div()
                .text_size(tokens::typography::FONT_XS)
                .text_color(theme::colors(cx).text_secondary)
                .child(message),
        )
        .child(
            div()
                .text_size(tokens::typography::FONT_XS)
                .text_color(theme::colors(cx).text_tertiary)
                .child(detail),
        )
}
