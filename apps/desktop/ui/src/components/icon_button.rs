use gpui::{ElementId, Stateful, prelude::*, px};

use super::{Icon, SurfaceVariant, surface};
use crate::{components::IconName, theme};

pub(crate) fn icon_button(id: impl Into<ElementId>, icon: IconName) -> Stateful<gpui::Div> {
    surface(SurfaceVariant::Control)
        .id(id)
        .size(px(28.0))
        .flex()
        .items_center()
        .justify_center()
        .occlude()
        .text_color(theme::muted())
        .child(Icon::new(icon))
}
