use super::*;
use crate::components::icon_button;

impl Editor {
    pub(super) fn render_header_left(
        &self,
        cx: &mut Context<Self>,
        compact_layout: bool,
    ) -> impl IntoElement {
        div()
            .w(px(if compact_layout { 200.0 } else { 320.0 }))
            .flex_shrink_0()
            .px(px(theme::CANVAS_GAP))
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .size(px(22.0))
                    .rounded(theme::radius(theme::Radius::Control))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(theme::accent())
                    .text_color(theme::brand_text())
                    .text_size(px(11.0))
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
    div()
        .id(id)
        .size(px(28.0))
        .rounded(theme::radius(theme::Radius::Control))
        .flex()
        .items_center()
        .justify_center()
        .occlude()
        .bg(theme::title_bar())
        .hover(|style| style.bg(theme::surface_hover()))
        .child(controls::PanelToggleIcon::new(position, state))
}
