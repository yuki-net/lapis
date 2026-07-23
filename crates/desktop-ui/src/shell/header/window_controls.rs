use super::*;

impl Editor {
    pub(super) fn render_window_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .h_full()
            .flex_shrink_0()
            .flex()
            .items_end()
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
