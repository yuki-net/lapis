mod center;
mod controls;
mod drag;
mod left;
mod menu;
mod menu_actions;
mod menu_definition;
mod right;
mod window_controls;

use drag::apply_drag_region;

use super::*;

impl Editor {
    /// Composes the persistent application header, separating app content (left/center/right)
    /// from OS window controls (minimize/maximize/close).
    pub(super) fn render_header(
        &self,
        cx: &mut Context<Self>,
        compact_layout: bool,
    ) -> impl IntoElement {
        apply_drag_region(
            div()
                .h(px(theme::TITLE_BAR_HEIGHT))
                .w_full()
                .flex_shrink_0()
                .flex()
                .flex_row()
                .items_center()
                .bg(theme::title_bar())
                // Mac OS
                .when(cfg!(target_os = "macos"), |this| {
                    this.child(div().w(px(80.0)).flex_shrink_0())
                        .child(self.render_app_header(cx, compact_layout))
                })
                // Windows and Linux
                .when(!cfg!(target_os = "macos"), |this| {
                    this.child(self.render_app_header(cx, compact_layout))
                        .child(self.render_window_controls(cx))
                }),
        )
    }

    /// Composes the 3-section application header (Left, Center, Right) with consistent padding.
    fn render_app_header(&self, cx: &mut Context<Self>, compact_layout: bool) -> impl IntoElement {
        div()
            .flex_1()
            .h_full()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .p(px(theme::CANVAS_GAP))
            .child(self.render_header_left(cx, compact_layout))
            .child(self.render_header_center())
            .child(self.render_header_right(cx))
    }
}
