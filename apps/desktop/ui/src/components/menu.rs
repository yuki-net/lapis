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
pub(crate) fn menu_surface(id: impl Into<ElementId>, cx: &gpui::App) -> Stateful<gpui::Div> {
    floating_panel(id, SurfaceVariant::Menu, cx)
        .flex()
        .flex_col()
}

pub(crate) fn menu_item(spec: MenuItemSpec, cx: &gpui::App) -> Stateful<gpui::Div> {
    let id = spec.id.clone();
    let enabled = spec.enabled;
    div()
        .id(ElementId::Name(id))
        .h(px(30.0))
        .flex_shrink_0()
        .w_full()
        .px(tokens::spacing::SM)
        .rounded(tokens::radius::MENU_ITEM)
        .flex()
        .items_center()
        .gap(tokens::spacing::SM)
        .text_size(px(13.0))
        .whitespace_nowrap()
        .text_color(if enabled {
            theme::colors(cx).text_primary
        } else {
            theme::colors(cx).text_tertiary
        })
        .when(enabled, |item| {
            item.hover(|style| style.bg(theme::colors(cx).button_background_hover))
        })
        .child(
            div()
                .size_4()
                .flex_shrink_0()
                .when_some(spec.icon, |slot, icon| slot.child(Icon::new(icon))),
        )
        .child(spec.label)
        .child(div().flex_1())
        .when_some(spec.shortcut, |item, shortcut| {
            item.child(
                div()
                    .text_color(theme::colors(cx).text_secondary)
                    .child(shortcut),
            )
        })
}
