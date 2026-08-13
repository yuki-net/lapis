use super::*;

impl Editor {
    /// フッター（ステータスバー）を描画する。
    pub(super) fn render_footer(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let display_name = self.session.display_name();
        let status_is_error = self.status.contains("失敗");
        let (line, column) = self.cursor_line_column();

        div()
            .h(px(31.0))
            .w_full()
            .flex_shrink_0()
            .px(px(14.0))
            .flex()
            .items_center()
            .gap_3()
            .text_size(px(12.0))
            .text_color(theme::subtle())
            .child("lapis")
            .child("›")
            .child(div().text_color(theme::muted()).child(display_name.clone()))
            .child("·")
            .child(div().text_color(rgb(0x8da8ff)).child("✓ Note"))
            .child(format!("R{}", self.session.revision()))
            .child(format!("Ln {line}, Col {column}"))
            .child("·")
            .child(
                div()
                    .text_color(if status_is_error {
                        rgb(0xf18f96)
                    } else {
                        theme::subtle()
                    })
                    .child(self.status.clone()),
            )
    }
}
