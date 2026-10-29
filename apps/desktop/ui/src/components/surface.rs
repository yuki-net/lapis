use gpui::{div, prelude::*};

use crate::{theme, tokens};

/// タブの有無によらない、通常PanelとInspector共通のアイランド。
pub(crate) fn panel_surface(cx: &gpui::App) -> gpui::Div {
    surface(SurfaceVariant::Panel, cx)
        .relative()
        .size_full()
        .flex()
        .flex_col()
        .min_w(gpui::px(0.))
        .min_h(gpui::px(0.))
        .overflow_hidden()
}

pub(crate) fn panel_header() -> gpui::Div {
    div()
        .h(gpui::px(34.))
        .px(tokens::spacing::XS)
        .py(gpui::px(3.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(tokens::spacing::XS)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SurfaceVariant {
    Menu,
    Panel,
    Popover,
    Control,
}

pub(crate) fn surface(variant: SurfaceVariant, cx: &gpui::App) -> gpui::Div {
    let mut surface = div().bg(match variant {
        SurfaceVariant::Menu | SurfaceVariant::Popover => theme::colors(cx).floating_background,
        SurfaceVariant::Panel => theme::colors(cx).background_secondary,
        SurfaceVariant::Control => theme::colors(cx).background_primary,
    });

    surface = match variant {
        SurfaceVariant::Menu | SurfaceVariant::Popover => surface
            .rounded(tokens::radius::MENU)
            .border_1()
            .border_color(theme::colors(cx).floating_border)
            .shadow_lg(),
        SurfaceVariant::Panel => surface.rounded(tokens::radius::PANEL),
        SurfaceVariant::Control => surface.rounded(tokens::radius::CONTROL),
    };

    surface.text_color(theme::colors(cx).text_primary).when(
        matches!(variant, SurfaceVariant::Control),
        |surface| {
            surface.hover(|style| {
                style
                    .bg(theme::colors(cx).button_background_hover)
                    .text_color(theme::colors(cx).text_primary)
            })
        },
    )
}
