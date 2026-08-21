use gpui::{ElementId, SharedString, Stateful, div, prelude::*, px};

use super::{Icon, IconName, SurfaceVariant, floating_surface};
use crate::theme;

#[derive(Clone)]
pub(crate) struct MenuItemSpec {
    pub(crate) id: SharedString,
    pub(crate) label: SharedString,
    pub(crate) shortcut: Option<SharedString>,
    pub(crate) icon: Option<IconName>,
    pub(crate) enabled: bool,
}

/// Shared outer surface for header menus, settings menus, and future context menus.
pub(crate) fn menu_surface(id: impl Into<ElementId>) -> Stateful<gpui::Div> {
    floating_surface(id, SurfaceVariant::Menu)
}

pub(crate) fn menu_item(spec: MenuItemSpec) -> Stateful<gpui::Div> {
    let id = spec.id.clone();
    let enabled = spec.enabled;
    div()
        .id(ElementId::Name(id))
        .h(px(32.0))
        .w_full()
        .px(theme::spacing(theme::Spacing::Sm))
        .rounded(theme::radius(theme::Radius::MenuItem))
        .flex()
        .items_center()
        .gap(theme::spacing(theme::Spacing::Sm))
        .text_size(px(13.0))
        .text_color(if enabled {
            theme::text()
        } else {
            theme::subtle()
        })
        .when(enabled, |item| {
            item.hover(|style| style.bg(theme::surface_hover()))
        })
        .when_some(spec.icon, |item, icon| item.child(Icon::new(icon)))
        .child(spec.label)
        .child(div().flex_1())
        .when_some(spec.shortcut, |item, shortcut| {
            item.child(div().text_color(theme::muted()).child(shortcut))
        })
}
