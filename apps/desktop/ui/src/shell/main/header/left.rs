use super::*;
use crate::components::{header_button, icon_button};

impl Shell {
    pub(super) fn render_header_left(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let panel_controls = div().flex().items_center().gap(tokens::spacing::XS).when(
            self.shell.show_panel_controls,
            |controls| {
                controls
                    .child(
                        panel_toggle_button(
                            "toggle-left-panel",
                            controls::PanelPosition::Left,
                            panel_state(
                                self.shell
                                    .layout
                                    .zone_open(crate::extension_ui::PanelPosition::Left),
                            ),
                        )
                        .on_click(cx.listener(
                            |this, event: &ClickEvent, window, cx| {
                                this.shell.tool_picker_anchor = event.position();
                                this.toggle_header_panel(
                                    crate::extension_ui::PanelPosition::Left,
                                    window,
                                    cx,
                                );
                            },
                        )),
                    )
                    .child(
                        panel_toggle_button(
                            "toggle-bottom-panel",
                            controls::PanelPosition::Bottom,
                            panel_state(
                                self.shell
                                    .layout
                                    .zone_open(crate::extension_ui::PanelPosition::Bottom),
                            ),
                        )
                        .on_click(cx.listener(
                            |this, event: &ClickEvent, window, cx| {
                                this.shell.tool_picker_anchor = event.position();
                                this.toggle_header_panel(
                                    crate::extension_ui::PanelPosition::Bottom,
                                    window,
                                    cx,
                                );
                            },
                        )),
                    )
                    .child(
                        panel_toggle_button(
                            "toggle-right-panel",
                            controls::PanelPosition::Right,
                            panel_state(
                                self.shell
                                    .layout
                                    .zone_open(crate::extension_ui::PanelPosition::Right),
                            ),
                        )
                        .on_click(cx.listener(
                            |this, event: &ClickEvent, window, cx| {
                                this.shell.tool_picker_anchor = event.position();
                                this.toggle_header_panel(
                                    crate::extension_ui::PanelPosition::Right,
                                    window,
                                    cx,
                                );
                            },
                        )),
                    )
            },
        );

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
                    .bg(theme::colors().text_accent)
                    .text_color(theme::colors().text_primary)
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
            .child(panel_controls)
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
