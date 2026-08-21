use super::*;
use crate::components::icon_button;

impl Editor {
    pub(super) fn render_window_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .h_full()
            .w(px(theme::WINDOW_CONTROL_WIDTH * 3.0 + 28.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .child(
                icon_button("open-settings-menu", IconName::Settings).on_click(cx.listener(
                    |this, event: &ClickEvent, _, cx| {
                        this.toggle_settings_menu(event.position(), cx);
                    },
                )),
            )
            .child(
                controls::window_control_button(
                    "window-minimize",
                    controls::WindowControlIconName::Minimize,
                    WindowControlArea::Min,
                    false,
                )
                .on_click(cx.listener(|_, _, window, _| window.minimize_window())),
            )
            .child(
                controls::window_control_button(
                    "window-maximize",
                    controls::WindowControlIconName::Maximize,
                    WindowControlArea::Max,
                    false,
                )
                .on_click(cx.listener(|_, _, window, _| window.zoom_window())),
            )
            .child(
                controls::window_control_button(
                    "window-close",
                    controls::WindowControlIconName::Close,
                    WindowControlArea::Close,
                    true,
                )
                .on_click(cx.listener(|_, _, window, _| window.remove_window())),
            )
    }
}
