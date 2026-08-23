use super::*;

impl Editor {
    pub(crate) fn render_tool_picker(
        &self,
        position: PanelPosition,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let query = self.shell.tool_picker_query.trim().to_lowercase();
        let tools = self
            .feature_registry
            .tool_contributions(position)
            .into_iter()
            .filter(|contribution| {
                if query.is_empty() {
                    return true;
                }
                let title = self.locale.resolve(&contribution.title).to_lowercase();
                let view = contribution
                    .view
                    .as_ref()
                    .map(|view| view.as_str().to_lowercase())
                    .unwrap_or_default();
                title.contains(&query) || view.contains(&query)
            })
            .filter_map(|contribution| {
                Some((
                    contribution.view.clone()?,
                    self.locale.resolve(&contribution.title),
                    contribution.icon.as_str().to_owned(),
                ))
            })
            .collect::<Vec<_>>();

        crate::components::floating_tree(
            self.shell.tool_picker_anchor,
            point(px(-20.0), tokens::spacing::GAP),
        )
        .child(
            crate::components::floating_panel(
                "tool-picker",
                crate::components::SurfaceVariant::Popover,
            )
            .w(px(250.0))
            .max_h(px(520.0))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.close_tool_picker(cx);
            }))
            .scrollable(ScrollAxis::Vertical)
            .p(tokens::spacing::SM)
            .text_color(theme::colors().text_primary)
            .child(
                div()
                    .px(tokens::spacing::XS)
                    .py(px(2.0))
                    .rounded(px(5.0))
                    .border_1()
                    .border_color(theme::colors().button_border_focused)
                    .bg(theme::colors().background_tertiary)
                    .flex()
                    .items_center()
                    .gap(tokens::spacing::XS)
                    .child(crate::components::Icon::new(
                        crate::components::IconName::Search,
                    ))
                    .child(div().text_size(tokens::typography::FONT_SM).child(
                        if self.shell.tool_picker_query.is_empty() {
                            div()
                                .text_color(theme::colors().text_secondary)
                                .child("Search tools...")
                        } else {
                            div()
                                .text_color(theme::colors().text_primary)
                                .child(format!("{}|", self.shell.tool_picker_query))
                        },
                    )),
            )
            .child(div().h(px(1.0)).my_1().bg(theme::colors().border_default))
            .children(tools.into_iter().map(|(view, title, icon)| {
                div()
                    .id(ElementId::Name(
                        format!("tool-picker-{}", view.as_str()).into(),
                    ))
                    .w_full()
                    .px(tokens::spacing::XS)
                    .py(px(2.0))
                    .rounded(tokens::radius::MENU_ITEM)
                    .flex()
                    .items_center()
                    .gap(tokens::spacing::XS)
                    .hover(|style| style.bg(theme::colors().button_background_hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_tool_from_picker(position, view.clone(), cx);
                    }))
                    .child(icon)
                    .child(title)
            })),
        )
    }
}
