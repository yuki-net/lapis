use super::*;
use crate::components::icon_button;

impl Shell {
    pub(super) fn render_header_right(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
                icon_button("open-notifications", IconName::Bell)
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
            .child(
                icon_button("open-settings-menu", IconName::Settings, cx).on_click(cx.listener(
                    |this, event: &ClickEvent, _, cx| {
                        this.toggle_settings_menu(event.position(), cx);
                    },
                )),
            )
    }
}
