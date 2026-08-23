use crate::{
    components::{ScrollAxis, ScrollableElement, panel_empty_state},
    features::terminal::TerminalFeature,
    theme,
};
use gpui::{div, prelude::*, px};

pub(crate) fn render_content(terminal: &TerminalFeature) -> gpui::Div {
    if let Some(terminal) = terminal.session.terminals().last() {
        div().flex_1().min_h(px(0.0)).child(
            div()
                .id("terminal-output-scroll")
                .size_full()
                .scrollable(ScrollAxis::Vertical)
                .p_2()
                .font_family("Cascadia Mono")
                .text_size(px(11.0))
                .text_color(theme::colors().text_primary)
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
