use gpui::{ElementId, SharedString, Stateful, div, prelude::*, px};

use super::{Icon, IconName, SurfaceVariant, floating_panel};
use crate::{theme, tokens};

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
    floating_panel(id, SurfaceVariant::Menu)
}

pub(crate) fn menu_item(spec: MenuItemSpec) -> Stateful<gpui::Div> {
    let id = spec.id.clone();
    let enabled = spec.enabled;
    div()
        .id(ElementId::Name(id))
        .h(px(32.0))
        .w_full()
        .px(tokens::spacing::SM)
        .rounded(tokens::radius::MENU_ITEM)
        .flex()
        .items_center()
        .gap(tokens::spacing::SM)
        .text_size(tokens::typography::FONT_SM)
        .text_color(if enabled {
            theme::colors().text
        } else {
            theme::colors().subtle
        })
        .when(enabled, |item| {
            item.hover(|style| style.bg(theme::colors().surface_hover))
        })
        .when_some(spec.icon, |item, icon| item.child(Icon::new(icon)))
        .child(spec.label)
        .child(div().flex_1())
        .when_some(spec.shortcut, |item, shortcut| {
            item.child(div().text_color(theme::colors().muted).child(shortcut))
        })
}
