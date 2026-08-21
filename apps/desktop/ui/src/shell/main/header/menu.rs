use crate::{
    components::{MenuItemSpec, menu_item, menu_surface, separator},
    shell::HeaderMenuSection as MenuId,
};
use gpui::ElementId;

use super::*;
use menu_definition::{MenuItemDefinition, ROOT_MENUS, items};

impl Editor {
    pub(crate) fn render_header_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.shell.header_menu_section;
        anchored()
            .position(self.shell.header_menu_anchor)
            .offset(point(px(-8.0), px(8.0)))
            .snap_to_window_with_margin(px(8.0))
            .child(
                div()
                    .id("header-menu-surface")
                    .relative()
                    .w(px(440.0))
                    .h(px(164.0))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.close_header_menu(cx);
                    }))
                    .child(
                        menu_surface("header-menu-column")
                            .w(px(190.0))
                            .h(px(164.0))
                            .p(theme::spacing(theme::Spacing::Xs))
                            .children(ROOT_MENUS.iter().map(|(menu, label)| {
                                self.render_header_menu_root(
                                    *menu,
                                    label,
                                    active == Some(*menu),
                                    cx,
                                )
                            })),
                    )
                    .when_some(active, |surface, menu| {
                        surface.child(self.render_header_submenu(menu, cx))
                    }),
            )
    }

    fn render_header_menu_root(
        &self,
        menu: MenuId,
        label: &'static str,
        active: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id(ElementId::Name(format!("header-menu-root-{label}").into()))
            .h(px(30.0))
            .w_full()
            .px_2()
            .rounded(px(4.0))
            .flex()
            .items_center()
            .justify_between()
            .text_size(px(13.0))
            .when(active, |item| item.bg(theme::accent_soft()))
            .hover(|style| style.bg(theme::accent_soft()))
            .child(label)
            .child(div().text_color(theme::muted()).child("›"))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.select_header_menu(menu, cx);
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.select_header_menu(menu, cx);
            }))
    }

    fn render_header_submenu(
        &self,
        menu: MenuId,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let children = items(menu)
            .iter()
            .flat_map(|item| {
                let separator = item
                    .separator_before
                    .then(|| separator().into_any_element());
                separator.into_iter().chain(std::iter::once(
                    self.render_header_item(item, cx).into_any_element(),
                ))
            })
            .collect::<Vec<_>>();

        menu_surface(ElementId::Name(format!("header-submenu-{menu:?}").into()))
            .absolute()
            .left(px(184.0))
            .top(px(0.0))
            .w(px(250.0))
            .p(theme::spacing(theme::Spacing::Xs))
            .children(children)
    }

    fn render_header_item(
        &self,
        item: &'static MenuItemDefinition,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let action = item.action;
        let id = item.id;
        let enabled = item.enabled;
        menu_item(MenuItemSpec {
            id: format!("header-menu-item-{id}").into(),
            label: item.label.into(),
            shortcut: item.shortcut.map(Into::into),
            icon: None,
            enabled,
        })
        .when(enabled, |row| {
            row.on_click(cx.listener(move |this, _, window, cx| {
                if let Some(action) = action {
                    this.execute_header_menu_action(action, window, cx);
                }
            }))
        })
    }
}
