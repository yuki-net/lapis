use crate::theme;
use gpui::{Context, Render, Window, div, prelude::*, px};
pub(crate) struct PreviewFeature {
    text: String,
}
impl PreviewFeature {
    pub(crate) fn new() -> Self {
        Self {
            text: String::new(),
        }
    }
    pub(crate) fn update(&mut self, text: String, cx: &mut Context<Self>) {
        self.text = text;
        cx.notify();
    }
}
impl Render for PreviewFeature {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .p_3()
            .children(self.text.lines().map(|line| {
                let (text, size) = if let Some(text) = line.strip_prefix("# ") {
                    (text, 24.0)
                } else if let Some(text) = line.strip_prefix("## ") {
                    (text, 19.0)
                } else {
                    (line, 13.0)
                };
                div()
                    .min_h(px(22.0))
                    .text_size(px(size))
                    .text_color(theme::colors().text_primary)
                    .child(text.to_owned())
            }))
    }
}
