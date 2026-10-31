use super::*;

impl Shell {
    pub(super) fn render_header_center(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(0.0))
            .min_w(px(0.0))
            .px(tokens::spacing::GAP)
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .gap(tokens::spacing::XS)
            .text_size(tokens::typography::FONT_SM)
            .child(
                crate::components::header_button(
                    "header-workspace",
                    self.session
                        .root
                        .as_deref()
                        .and_then(|root| root.file_name())
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| self.session.workspace_name().to_owned()),
                    cx,
                )
                .on_click(cx.listener(|_, _, _, cx| cx.notify())),
            )
            .child(Icon::new(IconName::ChevronRight).size(px(12.)))
            .when_some(self.branch_picker.clone(), |header, picker| {
                header.child(picker)
            })
    }
}
