use super::*;
use crate::components::{Popup, PopupPlacement, PopupScope, icon_button};

impl Shell {
    pub(super) fn render_header_right(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let scope = PopupScope::default();
        let settings = Popup::new(
            icon_button("open-settings-menu", IconName::Settings, cx)
                .debug_selector(|| "settings-menu-trigger".into())
                .when(self.shell.settings_menu_open, |button| {
                    button.bg(theme::colors(cx).button_background_selected)
                })
                .on_click(cx.listener(|this, _, window, cx| {
                    window.focus(&this.focus_handle);
                    this.toggle_settings_menu(cx);
                })),
            PopupPlacement::BelowStart,
            scope,
        )
        .on_dismiss(cx.listener(|this, _: &(), window, cx| {
            this.close_settings_menu(cx);
            window.focus(&this.focus_handle);
        }))
        .when(self.shell.settings_menu_open, |popup| {
            popup.content(self.render_settings_menu(cx))
        });
        div()
            .h_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1()
            .child(
                icon_button("open-search", IconName::Search, cx)
                    .debug_selector(|| "header-search".into())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.open_quick_search(window, cx)),
                    ),
            )
            .child(
                icon_button("open-notifications", IconName::Bell, cx)
                    .debug_selector(|| "header-notifications".into())
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.focus_view_in_zone(
                            ZonePosition::Right,
                            id::VIEW_NOTIFICATIONS.into(),
                            window,
                            cx,
                        );
                    })),
            )
            .child(settings)
    }
}
