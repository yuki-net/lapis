use super::*;

impl Shell {
    pub(crate) fn render_tool_picker(
        &self,
        target: PickerTarget,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let tools = self
            .picker_contributions(&target)
            .into_iter()
            .filter_map(|contribution| {
                Some((
                    contribution.view.clone()?,
                    self.locale.resolve(&contribution.title),
                    contribution.icon.as_str().to_owned(),
                    contribution
                        .category
                        .as_ref()
                        .map(|category| self.locale.resolve(category)),
                ))
            })
            .collect::<Vec<_>>();
        let selected = self.shell.tool_picker_selected;

        let picker_content = div()
            .p(tokens::spacing::SM)
            .text_color(theme::colors(cx).text_primary)
            .child(
                div()
                    .px(tokens::spacing::XS)
                    .py(px(2.0))
                    .rounded(px(5.0))
                    .border_1()
                    .border_color(theme::colors(cx).button_border_focused)
                    .bg(theme::colors(cx).background_tertiary)
                    .flex()
                    .items_center()
                    .gap(tokens::spacing::XS)
                    .child(crate::components::Icon::new(
                        crate::components::IconName::Search,
                    ))
                    .child(div().text_size(tokens::typography::FONT_SM).child(
                        if self.shell.tool_picker_query.is_empty() {
                            div()
                                .text_color(theme::colors(cx).text_secondary)
                                .child("Search tools...")
                        } else {
                            div()
                                .text_color(theme::colors(cx).text_primary)
                                .child(format!("{}|", self.shell.tool_picker_query))
                        },
                    )),
            )
            .child(div().h(px(1.0)).my_1().bg(theme::colors(cx).border_default))
            .children(tools.into_iter().enumerate().map(
                |(index, (view, title, icon, category))| {
                    let target = target.clone();
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
                        .when(index == selected, |item| {
                            item.bg(theme::colors(cx).button_background_selected)
                        })
                        .hover(|style| style.bg(theme::colors(cx).button_background_hover))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select_tool_from_picker(target.clone(), view.clone(), window, cx);
                        }))
                        .child(Icon::new(IconName::for_tool(&icon)))
                        .when_some(category, |item, category| {
                            item.child(
                                div()
                                    .text_size(tokens::typography::FONT_XS)
                                    .text_color(theme::colors(cx).text_tertiary)
                                    .child(format!("{category} · ")),
                            )
                        })
                        .child(title)
                },
            ));

        crate::components::floating_tree(
            self.shell.tool_picker_anchor,
            point(px(0.0), tokens::spacing::GAP),
        )
        .child(
            crate::components::floating_panel(
                "tool-picker",
                crate::components::SurfaceVariant::Popover,
                cx,
            )
            .w(px(250.0))
            .max_h(px(520.0))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                this.close_tool_picker(window, cx);
            }))
            .child(
                scroll_viewport(
                    "tool-picker-scroll",
                    ScrollAxis::Vertical,
                    &self.scroll_states.tool_picker,
                    picker_content,
                    cx,
                )
                .max_h(px(520.0)),
            ),
        )
    }
}
