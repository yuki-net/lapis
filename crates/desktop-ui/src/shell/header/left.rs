use super::*;

impl Editor {
    pub(super) fn render_header_left(
        &self,
        cx: &mut Context<Self>,
        compact_layout: bool,
    ) -> impl IntoElement {
        div()
            .w(px(if compact_layout { 200.0 } else { 320.0 }))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .size(px(22.0))
                    .rounded(px(6.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(theme::accent())
                    .text_color(rgb(0xffffff))
                    .text_size(px(11.0))
                    .child("L"),
            )
            .child(
                div()
                    .id("header-menu")
                    .size(px(28.0))
                    .rounded(px(6.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .occlude()
                    .bg(theme::title_bar())
                    .hover(|style| style.bg(theme::surface_hover()))
                    .child(controls::menu_icon()),
            )
            .child(
                panel_toggle_button(
                    "toggle-left-panel",
                    controls::PanelPosition::Left,
                    controls::PanelState::Open,
                ),
            )
            .child(
                panel_toggle_button(
                    "toggle-bottom-panel",
                    controls::PanelPosition::Bottom,
                    if self.shell.bottom_panel_open {
                        controls::PanelState::Open
                    } else {
                        controls::PanelState::Close
                    },
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.toggle_bottom_panel(&ToggleBottomPanel, window, cx);
                })),
            )
            .child(
                panel_toggle_button(
                    "toggle-right-panel",
                    controls::PanelPosition::Right,
                    if self
                        .shell
                        .side_panel
                        .as_ref()
                        .is_some_and(|view| view.as_str() == id::VIEW_PREVIEW)
                    {
                        controls::PanelState::Open
                    } else {
                        controls::PanelState::Close
                    },
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.toggle_preview(&TogglePreview, window, cx);
                })),
            )
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
        .rounded(px(6.0))
        .flex()
        .items_center()
        .justify_center()
        .occlude()
        .bg(theme::title_bar())
        .hover(|style| style.bg(theme::surface_hover()))
        .child(controls::PanelToggleIcon::new(position, state))
}
