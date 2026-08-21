use gpui::{ElementId, Stateful, div, prelude::*};

use crate::theme;

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
        SurfaceVariant::Menu | SurfaceVariant::Popover => theme::surface(),
        SurfaceVariant::Panel => theme::island(),
        SurfaceVariant::Control | SurfaceVariant::Tab => theme::title_bar(),
    });

    surface = match variant {
        SurfaceVariant::Menu | SurfaceVariant::Popover => surface
            .rounded(theme::radius(theme::Radius::Menu))
            .border_1()
            .border_color(theme::border())
            .shadow_lg(),
        SurfaceVariant::Panel => surface.rounded(theme::radius(theme::Radius::Panel)),
        SurfaceVariant::Control => surface.rounded(theme::radius(theme::Radius::Control)),
        SurfaceVariant::Tab => surface.rounded_t(theme::radius(theme::Radius::Tab)),
    };

    surface
        .text_color(theme::text())
        .when(matches!(variant, SurfaceVariant::Control), |surface| {
            surface.hover(|style| style.bg(theme::surface_hover()).text_color(theme::text()))
        })
}

pub(crate) fn floating_surface(
    id: impl Into<ElementId>,
    variant: SurfaceVariant,
) -> Stateful<gpui::Div> {
    surface(variant).id(id)
}
