use gpui::{ElementId, Stateful, prelude::*};

use super::{Icon, SurfaceVariant, surface};
use crate::{components::IconName, theme, tokens};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ButtonSize {
    Xs,
    Sm,
    Md,
}

pub(crate) fn button(
    id: impl Into<ElementId>,
    content: impl IntoElement,
    size: ButtonSize,
) -> Stateful<gpui::Div> {
    let height = match size {
        ButtonSize::Xs => tokens::size::BUTTON_XS,
        ButtonSize::Sm => tokens::size::BUTTON_SM,
        ButtonSize::Md => tokens::size::BUTTON_MD,
    };

    surface(SurfaceVariant::Control)
        .id(id)
        .h(height)
        .px(tokens::spacing::XS)
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .occlude()
        .text_color(theme::colors().text_secondary)
        .child(content)
}

pub(crate) fn header_button(
    id: impl Into<ElementId>,
    content: impl IntoElement,
) -> Stateful<gpui::Div> {
    button(id, content, ButtonSize::Md)
}

pub(crate) fn icon_button(id: impl Into<ElementId>, icon: IconName) -> Stateful<gpui::Div> {
    header_button(id, Icon::new(icon)).w(tokens::size::HEADER_BUTTON)
}
