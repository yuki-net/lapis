use super::*;

impl Editor {
    pub(super) fn render_bottom_panel(&self, cx: &mut Context<Self>) -> gpui::Div {
        let bottom_tabs = self
            .feature_registry
            .contributions(UiSlot::BottomDock)
            .into_iter()
            .enumerate()
            .filter_map(|(index, contribution)| {
                Some((
                    index,
                    contribution.view.clone()?,
                    self.locale.resolve(&contribution.title),
                ))
            })
            .collect::<Vec<_>>();
        div()
            .h(px(self.shell.bottom_panel_height))
            .flex_shrink_0()
            .overflow_hidden()
            .rounded(px(theme::ISLAND_RADIUS))
            .border_1()
            .border_color(theme::border())
            .bg(theme::island())
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(36.0))
                    .flex_shrink_0()
                    .px_2()
                    .border_b_1()
                    .border_color(theme::border())
                    .flex()
                    .items_center()
                    .gap_1()
                    .children(bottom_tabs.into_iter().map(|(index, view, label)| {
                        let active = self.shell.bottom_panel == view;
                        panel_tab(index, label, active).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.shell.bottom_panel = view.clone();
                                this.refresh_feature_activation();
                                cx.notify();
                            },
                        ))
                    }))
                    .child(div().flex_1())
                    .when(
                        self.shell.bottom_panel.as_str() == id::VIEW_TERMINAL,
                        |bar| {
                            bar.child(
                                task_action_button("Start", false).on_click(
                                    cx.listener(|this, _, _, cx| this.start_terminal(cx)),
                                ),
                            )
                            .child(
                                task_action_button("Send clipboard", false).on_click(
                                    cx.listener(|this, _, _, cx| this.send_terminal_clipboard(cx)),
                                ),
                            )
                        },
                    )
                    .child(
                        div()
                            .id("close-bottom-panel")
                            .size(px(25.0))
                            .rounded(px(5.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(theme::muted())
                            .hover(|style| {
                                style.bg(theme::surface_hover()).text_color(theme::text())
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.shell.bottom_panel_open = false;
                                this.refresh_feature_activation();
                                cx.notify();
                            }))
                            .child("×"),
                    ),
            )
            .child(match self.shell.bottom_panel.as_str() {
                id::VIEW_TERMINAL => self.render_terminal_content(),
                id::VIEW_PROBLEMS => self.render_problems_content(),
                id::VIEW_OUTPUT => self.render_output_content(),
                _ => panel_empty_state(
                    "?",
                    "Unknown view",
                    self.shell.bottom_panel.as_str().to_owned(),
                ),
            })
    }
}
