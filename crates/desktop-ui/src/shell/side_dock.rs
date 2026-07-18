use super::*;

impl Editor {
    pub(super) fn render_side_panel(&self, cx: &mut Context<Self>) -> gpui::Div {
        let panel = self
            .shell
            .side_panel
            .clone()
            .unwrap_or_else(|| ViewId::new(id::VIEW_PREVIEW));
        let contribution = self
            .feature_registry
            .contributions(UiSlot::SideDock)
            .into_iter()
            .find(|item| item.view.as_ref() == Some(&panel));
        let title = contribution
            .map(|item| self.locale.resolve(&item.title))
            .unwrap_or_else(|| panel.as_str().to_owned());
        let icon = contribution
            .map(|item| self.icon_theme.resolve(&item.icon))
            .unwrap_or_else(|| "·".to_owned());
        let is_assistant = panel.as_str() == id::VIEW_ASSISTANT;
        let is_command_search = panel.as_str() == id::VIEW_COMMAND_SEARCH;

        div()
            .w(px(self.shell.side_panel_width))
            .h_full()
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
                    .h(px(39.0))
                    .flex_shrink_0()
                    .px_2()
                    .border_b_1()
                    .border_color(theme::border())
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(12.0))
                    .text_color(theme::text())
                    .child(
                        div()
                            .text_color(if is_assistant {
                                rgb(0xb8b9f8)
                            } else {
                                theme::muted()
                            })
                            .child(icon),
                    )
                    .child(title)
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("close-side-panel")
                            .size(px(25.0))
                            .rounded(px(5.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(theme::muted())
                            .hover(|style| {
                                style.bg(theme::surface_hover()).text_color(theme::text())
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.shell.side_panel = None;
                                this.refresh_feature_activation();
                                window.focus(&this.focus_handle);
                                cx.notify();
                            }))
                            .child("×"),
                    ),
            )
            .when_else(
                is_command_search,
                |panel| panel.child(self.quick_search.clone()),
                |panel| panel,
            )
            .when_else(
                panel.as_str() == id::VIEW_PREVIEW && !is_command_search,
                |panel| {
                    panel.child(
                        div()
                            .id("preview-scroll")
                            .h(px(0.0))
                            .min_h(px(0.0))
                            .flex_1()
                            .overflow_y_scroll()
                            .p_4()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .when_else(
                                self.session.is_empty(),
                                |preview| {
                                    preview.child(panel_empty_state(
                                        "◫",
                                        "プレビューする内容がありません",
                                        "Markdown を入力するとここに反映されます",
                                    ))
                                },
                                |preview| preview.children(self.preview_lines()),
                            ),
                    )
                },
                |panel| {
                    if is_command_search {
                        panel
                    } else {
                        panel.child(self.render_assistant_content(cx))
                    }
                },
            )
    }
}
