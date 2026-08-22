mod center;
mod controls;
mod drag;
mod left;
mod menu;
mod menu_actions;
mod menu_definition;
mod window_controls;

use drag::apply_drag_region;

use super::*;

impl Editor {
    /// Composes the persistent application header from its four regions.
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
                .pl(px(theme::CANVAS_GAP))
                .flex()
                .flex_row()
                .items_center()
                .bg(theme::title_bar())
                .when(cfg!(target_os = "macos"), |this| {
                    this.child(div().w(px(80.0)).flex_shrink_0())
                })
                .child(self.render_header_left(cx, compact_layout))
                .child(self.render_header_center())
                .when(!cfg!(target_os = "macos"), |this| {
                    this.child(self.render_window_controls(cx))
                }),
        )
    }
}
