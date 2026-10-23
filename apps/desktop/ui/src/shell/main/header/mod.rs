mod center;
pub(super) mod controls;
mod drag;
mod left;
mod menu;
mod menu_actions;
mod menu_definition;
mod right;
mod window_controls;

use drag::apply_drag_region;

use super::*;

impl Shell {
    /// Composes the persistent application header, separating app content (left/center/right)
    /// from OS window controls (minimize/maximize/close).
    pub(super) fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(all(feature = "hot-reload", debug_assertions))]
        {
            subsecond::HotFn::current(Self::render_header_hot).call((self, cx))
        }

        #[cfg(not(all(feature = "hot-reload", debug_assertions)))]
        {
            self.render_header_content(cx)
        }
    }

    #[cfg(all(feature = "hot-reload", debug_assertions))]
    #[inline(never)]
    fn render_header_hot(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        self.render_header_content(cx).into_any_element()
    }

    fn render_header_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        apply_drag_region(
            div()
                .h(tokens::size::TITLE_BAR_HEIGHT)
                .w_full()
                .flex_shrink_0()
                .flex()
                .flex_row()
                .items_center()
                .bg(theme::colors().background_primary)
                // Mac OS
                .when(cfg!(target_os = "macos"), |this| {
                    this.child(div().w(px(80.0)).flex_shrink_0())
                        .child(self.render_app_header(cx))
                })
                // Windows and Linux
                .when(!cfg!(target_os = "macos"), |this| {
                    this.child(self.render_app_header(cx))
                        .child(self.render_window_controls(cx))
                }),
        )
    }

    /// Renders the application header sections, or only the workspace title in detached windows.
    fn render_app_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
