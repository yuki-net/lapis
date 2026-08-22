use super::*;

impl Editor {
    pub(super) fn render_header_center(&self) -> impl IntoElement {
        div()
            .w(px(0.0))
            .min_w(px(0.0))
            .h(px(
                theme::TITLE_BAR_HEIGHT - theme::WINDOW_RESIZE_BORDER_HEIGHT
            ))
            .mt(px(theme::WINDOW_RESIZE_BORDER_HEIGHT))
            .px(px(theme::CANVAS_GAP))
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .gap_2()
            .text_size(px(12.0))
            .child(
                div()
                    .text_color(theme::muted())
                    .child(self.session.workspace_name().to_owned()),
            )
            .child(div().text_color(theme::subtle()).child("›"))
            .child(div().text_color(theme::subtle()).child("local"))
    }
}
