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

        crate::components::floating_tree(self.shell.tool_picker_anchor, point(px(-20.0), px(8.0)))
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
                .p(theme::spacing(theme::Spacing::Sm))
                .text_color(theme::text())
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .rounded(px(5.0))
                        .border_1()
                        .border_color(theme::command_input_border())
                        .bg(theme::surface())
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(crate::components::Icon::new(
                            crate::components::IconName::Search,
                        ))
                        .child(div().text_size(px(12.0)).child(
                            if self.shell.tool_picker_query.is_empty() {
                                div().text_color(theme::muted()).child("Search tools...")
                            } else {
                                div()
                                    .text_color(theme::text())
                                    .child(format!("{}|", self.shell.tool_picker_query))
                            },
                        )),
                )
                .child(div().h(px(1.0)).my_1().bg(theme::border()))
                .children(tools.into_iter().map(|(view, title, icon)| {
                    div()
                        .id(ElementId::Name(
                            format!("tool-picker-{}", view.as_str()).into(),
                        ))
                        .w_full()
                        .px_2()
                        .py_1()
                        .rounded(theme::radius(theme::Radius::MenuItem))
                        .flex()
                        .items_center()
                        .gap_2()
                        .hover(|style| style.bg(theme::surface_hover()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_tool_from_picker(position, view.clone(), cx);
                        }))
                        .child(icon)
                        .child(title)
                })),
            )
    }
}
