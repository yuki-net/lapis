mod center;
pub(super) mod controls;
mod left;
mod menu;
mod menu_actions;
mod menu_definition;
mod right;

use super::*;

impl Shell {
    /// Composes the persistent application header, separating app content (left/center/right)
    /// from OS window controls (minimize/maximize/close).
    pub(super) fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        crate::window_host::chrome::title_bar(
            self.render_app_header(cx),
            cx.listener(|_, _, _, cx| cx.emit(UiCommand::CloseWindow)),
            cx,
        )
    }

    /// Renders the application header sections, or only the workspace title in detached windows.
    fn render_app_header(&self, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .flex_1()
            .h_full()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .p(tokens::spacing::GAP)
            .when(self.detached, |header| {
                header.child(format!("Lapis — {}", self.session.display_name()))
            })
            .when(!self.detached, |header| {
                header
                    .child(self.render_header_left(cx))
                    .child(self.render_header_center(cx))
                    .child(self.render_header_right(cx))
            })
    }
}
