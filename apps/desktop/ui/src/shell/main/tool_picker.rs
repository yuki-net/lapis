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

        anchored()
            .position(point(px(120.0), px(82.0)))
            .snap_to_window_with_margin(px(8.0))
            .child(
                crate::components::floating_surface(
                    "tool-picker",
                    crate::components::SurfaceVariant::Popover,
                )
                .w(px(250.0))
                .max_h(px(520.0))
                .overflow_y_scroll()
                .p(theme::spacing(theme::Spacing::Sm))
                .text_color(theme::text())
                .child(div().px_2().py_1().text_color(theme::muted()).child(
                    if self.shell.tool_picker_query.is_empty() {
                        "Search".to_owned()
                    } else {
                        self.shell.tool_picker_query.clone()
                    },
                ))
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
