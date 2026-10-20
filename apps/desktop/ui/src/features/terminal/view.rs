use super::TerminalFeature;
use crate::{
    components::{ScrollAxis, action_button, scroll_viewport},
    theme,
};
use gpui::{Context, Render, Window, div, prelude::*, px};
impl Render for TerminalFeature {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(
                        action_button("Start", true)
                            .on_click(cx.listener(|this, _, _, cx| this.start(cx))),
                    )
                    .child(
                        action_button("Clipboard", false)
                            .on_click(cx.listener(|this, _, _, cx| this.send_clipboard(cx))),
                    ),
            )
            .child(self.message.clone())
            .child(
                scroll_viewport(
                    "terminal-output",
                    ScrollAxis::Vertical,
                    &self.scroll,
                    div()
                        .p_2()
                        .font_family("Cascadia Mono")
                        .text_size(px(11.0))
                        .text_color(theme::colors().text_primary)
                        .child(
                            self.terminals
                                .last()
                                .map(|terminal| {
                                    String::from_utf8_lossy(&terminal.output).into_owned()
                                })
                                .unwrap_or_default(),
                        ),
                )
                .flex_1(),
            )
    }
}
