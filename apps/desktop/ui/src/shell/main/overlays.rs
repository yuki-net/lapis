use super::*;

impl Editor {
    pub(crate) fn render_settings_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let anchor = self.shell.settings_menu_anchor;
        anchored()
            .position(anchor)
            .offset(point(px(-250.0), px(8.0)))
            .snap_to_window_with_margin(px(8.0))
            .child(
                div()
                    .id("settings-menu")
                    .w(px(250.0))
                    .p_2()
                    .rounded(px(7.0))
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::surface())
                    .shadow_lg()
                    .text_color(theme::text())
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.close_settings_menu(cx);
                    }))
                    .child(
                        settings_menu_item(
                            IconName::Settings,
                            "Settings",
                            Some("Ctrl+,".to_owned()),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.open_settings_view(cx);
                        })),
                    )
                    .child(
                        settings_menu_item(
                            IconName::SunMoon,
                            "Theme",
                            theme::name(&theme::active_id()),
                        )
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
                    .text_color(theme::text())
                    .hover(|style| style.bg(theme::surface_hover()))
                    .when(selected, |item| item.bg(theme::accent_soft()))
                    .child(if selected { "✓" } else { "" })
                    .child(name)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_theme(click_id.clone(), cx);
                    }))
            }))
    }
}
