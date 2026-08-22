use super::*;
use crate::components::{MenuItemSpec, floating_panel, floating_tree, menu_item};

impl Editor {
    pub(crate) fn render_settings_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let anchor = self.shell.settings_menu_anchor;
        floating_tree(anchor, point(px(-250.0), px(8.0))).child(
            floating_panel("settings-menu", SurfaceVariant::Menu)
                .w(px(250.0))
                .p(tokens::spacing::SM)
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.close_settings_menu(cx);
                }))
                .child(
                    menu_item(MenuItemSpec {
                        id: "settings-menu-settings".into(),
                        label: "Settings".into(),
                        shortcut: Some("Ctrl+,".into()),
                        icon: Some(IconName::Settings),
                        enabled: true,
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.open_settings_view(cx);
                    })),
                )
                .child(
                    menu_item(MenuItemSpec {
                        id: "settings-menu-theme".into(),
                        label: "Theme".into(),
                        shortcut: theme::name(&theme::active_id()).map(Into::into),
                        icon: Some(IconName::SunMoon),
                        enabled: true,
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_theme_preference(cx);
                    })),
                )
                .when(self.shell.theme_picker_open, |menu| {
                    menu.child(self.render_theme_picker(cx))
                }),
        )
    }

    pub(super) fn render_theme_picker(&self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let active = theme::active_id();
        div()
            .id("theme-picker")
            .mt_1()
            .mb_1()
            .pl_2()
            .flex()
            .flex_col()
            .gap_1()
            .children(theme::available().into_iter().map(|(theme_id, name)| {
                let selected = active == theme_id;
                let click_id = theme_id.clone();
                div()
                    .id(gpui::SharedString::from(theme_id.as_str().to_owned()))
                    .h(px(30.0))
                    .w_full()
                    .px_2()
                    .rounded(px(5.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.0))
                    .text_color(theme::colors().text)
                    .hover(|style| style.bg(theme::colors().surface_hover))
                    .when(selected, |item| item.bg(theme::colors().accent_soft))
                    .child(if selected { "✓" } else { "" })
                    .child(name)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_theme(click_id.clone(), cx);
                    }))
            }))
    }
}
