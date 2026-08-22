use super::*;
use crate::components::{header_button, icon_button};

impl Editor {
    pub(super) fn render_header_left(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(tokens::spacing::XS)
            .child(
                div()
                    .size(tokens::size::BUTTON_XS)
                    .rounded(tokens::radius::CONTROL)
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(theme::colors().accent)
                    .text_color(theme::colors().brand_text)
                    .text_size(tokens::typography::FONT_XS)
                    .child("L"),
            )
            .child(
                icon_button("header-menu", IconName::Menu).on_click(cx.listener(
                    |this, event: &ClickEvent, _, cx| {
                        this.toggle_header_menu(event.position(), cx);
                    },
                )),
            )
            .child(
                panel_toggle_button(
                    "toggle-left-panel",
                    controls::PanelPosition::Left,
                    panel_state(self.shell.left_panel.open),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.toggle_header_panel(crate::extension_ui::PanelPosition::Left, cx);
                })),
            )
            .child(
                panel_toggle_button(
                    "toggle-bottom-panel",
                    controls::PanelPosition::Bottom,
                    if self.shell.bottom_panel.open {
                        controls::PanelState::Open
                    } else {
                        controls::PanelState::Close
                    },
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.toggle_header_panel(crate::extension_ui::PanelPosition::Bottom, cx);
                    let _ = window;
                })),
            )
            .child(
                panel_toggle_button(
                    "toggle-right-panel",
                    controls::PanelPosition::Right,
                    panel_state(self.shell.right_panel.open),
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.toggle_header_panel(crate::extension_ui::PanelPosition::Right, cx);
                    let _ = window;
                })),
            )
    }
}

fn panel_state(open: bool) -> controls::PanelState {
    if open {
        controls::PanelState::Open
    } else {
        controls::PanelState::Close
    }
}

fn panel_toggle_button(
    id: &'static str,
    position: controls::PanelPosition,
    state: controls::PanelState,
) -> gpui::Stateful<gpui::Div> {
    header_button(id, controls::PanelToggleIcon::new(position, state))
        .w(tokens::size::HEADER_BUTTON)
}
