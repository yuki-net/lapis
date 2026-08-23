use gpui::{SharedString, div, prelude::*};

use crate::{theme, tokens};

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
        .px(tokens::spacing::MD)
        .gap(tokens::spacing::XS)
        .text_center()
        .child(
            div()
                .size(tokens::size::HEADER_BUTTON)
                .rounded(tokens::radius::PANEL)
                .flex()
                .items_center()
                .justify_center()
                .bg(theme::colors().background_tertiary)
                .text_size(tokens::typography::FONT_LG)
                .text_color(theme::colors().text_secondary)
                .child(icon),
        )
        .child(
            div()
                .text_size(tokens::typography::FONT_SM)
                .text_color(theme::colors().text_primary)
                .child(title),
        )
        .child(
            div()
                .text_size(tokens::typography::FONT_XS)
                .text_color(theme::colors().text_secondary)
                .child(message),
        )
        .child(
            div()
                .text_size(tokens::typography::FONT_XS)
                .text_color(theme::colors().text_tertiary)
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
        .gap(tokens::spacing::XS)
        .px(tokens::spacing::LG)
        .text_center()
        .child(
            div()
                .text_size(tokens::typography::FONT_XL)
                .text_color(theme::colors().text_tertiary)
                .child(icon),
        )
        .child(
            div()
                .text_size(tokens::typography::FONT_XS)
                .text_color(theme::colors().text_secondary)
                .child(message),
        )
        .child(
            div()
                .text_size(tokens::typography::FONT_XS)
                .text_color(theme::colors().text_tertiary)
                .child(detail),
        )
}

pub(crate) fn panel_empty_state_element(
    icon: impl IntoElement,
    message: impl Into<SharedString>,
    detail: impl Into<SharedString>,
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
                .text_color(theme::colors().text_secondary)
                .child(message),
        )
        .child(
            div()
                .text_size(tokens::typography::FONT_XS)
                .text_color(theme::colors().text_tertiary)
                .child(detail),
        )
}
