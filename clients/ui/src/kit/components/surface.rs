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

/// Panel本文の左右余白を常時確保する共通定義。
/// Panel本文のScroll container自身へ適用し、内容の幅を制限しない。
/// Featureは左右paddingを個別に付けず、この内側へ描画する。
pub(crate) fn panel_content(body: gpui::Div) -> gpui::Div {
    body.px(tokens::spacing::PANEL_CONTENT_X)
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
            .when(variant == SurfaceVariant::Menu, |surface| {
                surface.shadow(vec![
                    gpui::BoxShadow {
                        color: gpui::rgba(0x00000066).into(),
                        offset: gpui::point(gpui::px(0.), gpui::px(8.)),
                        blur_radius: gpui::px(24.),
                        spread_radius: gpui::px(0.),
                        #[cfg(target_family = "wasm")]
                        inset: false,
                    },
                    gpui::BoxShadow {
                        color: gpui::rgba(0x00000040).into(),
                        offset: gpui::point(gpui::px(0.), gpui::px(2.)),
                        blur_radius: gpui::px(6.),
                        spread_radius: gpui::px(0.),
                        #[cfg(target_family = "wasm")]
                        inset: false,
                    },
                ])
            })
            .when(variant == SurfaceVariant::Popover, |surface| {
                surface.shadow_lg()
            }),
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
