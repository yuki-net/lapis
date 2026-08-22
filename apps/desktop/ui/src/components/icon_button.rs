use gpui::{ElementId, Stateful, prelude::*, px};

use super::{Icon, SurfaceVariant, surface};
use crate::{components::IconName, theme};

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
        ButtonSize::Xs => px(theme::BUTTON_HEIGHT_XS),
        ButtonSize::Sm => px(theme::BUTTON_HEIGHT_SM),
        ButtonSize::Md => px(theme::HEADER_BUTTON_SIZE),
    };

    surface(SurfaceVariant::Control)
        .id(id)
        .h(height)
        .px(theme::spacing(theme::Spacing::Xs))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .occlude()
        .text_color(theme::muted())
        .child(content)
}

pub(crate) fn header_button(
    id: impl Into<ElementId>,
    content: impl IntoElement,
) -> Stateful<gpui::Div> {
    button(id, content, ButtonSize::Md)
}

pub(crate) fn icon_button(id: impl Into<ElementId>, icon: IconName) -> Stateful<gpui::Div> {
    header_button(id, Icon::new(icon)).w(px(theme::HEADER_BUTTON_SIZE))
}
