use super::*;

impl Editor {
    pub(super) fn render_terminal_content(&self) -> gpui::Div {
        if let Some(terminal) = self.terminal.session.terminals().last() {
            div().flex_1().min_h(px(0.0)).child(
                div()
                    .id("terminal-output-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .p_2()
                    .font_family("Cascadia Mono")
                    .text_size(px(11.0))
                    .text_color(theme::text())
                    .child(terminal.output.clone()),
            )
        } else {
            panel_empty_state(
                ">_",
                "Terminal は停止中です",
                "Start で Workspace の PTY を起動します",
            )
        }
    }
}
