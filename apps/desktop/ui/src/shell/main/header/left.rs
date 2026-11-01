use super::*;
use crate::components::{Popup, PopupPlacement, PopupScope, header_button, icon_button};

impl Shell {
    pub(super) fn render_header_left(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let scope = PopupScope::default();
        let menu = Popup::new(
            icon_button("header-menu", IconName::Menu, cx)
                .debug_selector(|| "header-menu-trigger".into())
                .when(self.shell.header_menu_open, |button| {
                    button.bg(theme::colors(cx).button_background_selected)
                })
                .on_click(cx.listener(|this, _, window, cx| {
                    window.focus(&this.focus_handle);
                    this.toggle_header_popup(super::HeaderPopup::Main, cx);
                })),
            PopupPlacement::BelowStart,
            scope.clone(),
        )
        .on_dismiss(cx.listener(|this, _: &(), window, cx| {
            this.close_header_menu(cx);
            window.focus(&this.focus_handle);
        }))
        .when(self.shell.header_menu_open, |popup| {
            popup.content(self.render_header_menu(
                &scope,
                window.viewport_size().height - px(16.),
                cx,
            ))
        });
        let panel_controls = div().flex().items_center().gap(tokens::spacing::XS).when(
            self.shell.show_panel_controls,
            |controls| {
                controls
                    .child(
                        panel_toggle_button(
                            "toggle-left-zone",
                            controls::ZonePosition::Left,
                            panel_state(
                                self.shell
                                    .layout
                                    .zone_open(crate::extension_ui::ZonePosition::Left),
                            ),
                            cx,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.execute_command(id::COMMAND_TOGGLE_LEFT_ZONE.into(), window, cx);
                        })),
                    )
                    .child(
                        panel_toggle_button(
                            "toggle-bottom-zone",
                            controls::ZonePosition::Bottom,
                            panel_state(
                                self.shell
                                    .layout
                                    .zone_open(crate::extension_ui::ZonePosition::Bottom),
                            ),
                            cx,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.execute_command(id::COMMAND_TOGGLE_BOTTOM_ZONE.into(), window, cx);
                        })),
                    )
                    .child(
                        panel_toggle_button(
                            "toggle-right-zone",
                            controls::ZonePosition::Right,
                            panel_state(
                                self.shell
                                    .layout
                                    .zone_open(crate::extension_ui::ZonePosition::Right),
                            ),
                            cx,
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.execute_command(id::COMMAND_TOGGLE_RIGHT_ZONE.into(), window, cx);
                        })),
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
                    .bg(theme::colors(cx).text_accent)
                    .text_color(theme::colors(cx).text_primary)
                    .text_size(tokens::typography::FONT_XS)
                    .child("L"),
            )
            .child(menu)
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
    position: controls::ZonePosition,
    state: controls::PanelState,
    cx: &gpui::App,
) -> gpui::Stateful<gpui::Div> {
    header_button(id, controls::PanelToggleIcon::new(position, state), cx)
        .w(tokens::size::HEADER_BUTTON)
}
