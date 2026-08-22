use super::*;

impl Editor {
    pub(super) fn render_header_center(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(0.0))
            .min_w(px(0.0))
            .px(px(theme::CANVAS_GAP))
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .gap_1()
            .text_size(px(12.0))
            .child(
                crate::components::header_button(
                    "header-workspace",
                    self.session.workspace_name().to_owned(),
                )
                .on_click(cx.listener(|_, _, _, cx| cx.notify())),
            )
            .child(div().text_color(theme::subtle()).child("›"))
            .child(
                crate::components::header_button("header-branch", "make-develop")
                    .on_click(cx.listener(|_, _, _, cx| cx.notify())),
            )
    }
}
