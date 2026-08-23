use gpui::{div, prelude::*};

use crate::{theme, tokens};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SurfaceVariant {
    Menu,
    Panel,
    Popover,
    Control,
    Tab,
}

pub(crate) fn surface(variant: SurfaceVariant) -> gpui::Div {
    let mut surface = div().bg(match variant {
        SurfaceVariant::Menu | SurfaceVariant::Popover => theme::colors().floating_background,
        SurfaceVariant::Panel => theme::colors().background_secondary,
        SurfaceVariant::Control | SurfaceVariant::Tab => theme::colors().background_primary,
    });

    surface = match variant {
        SurfaceVariant::Menu | SurfaceVariant::Popover => surface
            .rounded(tokens::radius::MENU)
            .border_1()
            .border_color(theme::colors().floating_border)
            .shadow_lg(),
        SurfaceVariant::Panel => surface.rounded(tokens::radius::PANEL),
        SurfaceVariant::Control => surface.rounded(tokens::radius::CONTROL),
        SurfaceVariant::Tab => surface.rounded_t(tokens::radius::TAB),
    };

    surface.text_color(theme::colors().text_primary).when(
        matches!(variant, SurfaceVariant::Control),
        |surface| {
            surface.hover(|style| {
                style
                    .bg(theme::colors().button_background_hover)
                    .text_color(theme::colors().text_primary)
            })
        },
    )
}
