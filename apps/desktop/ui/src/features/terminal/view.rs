use crate::{
    components::{ScrollAxis, ScrollState, panel_empty_state, scroll_viewport},
    features::terminal::TerminalFeature,
    theme,
};
use gpui::{div, prelude::*, px};

pub(crate) fn render_content(terminal: &TerminalFeature, scroll_state: &ScrollState) -> gpui::Div {
    if let Some(terminal) = terminal.session.terminals().last() {
        div().flex_1().min_h(px(0.0)).child(
            scroll_viewport(
                "terminal-output-scroll",
                ScrollAxis::Vertical,
                scroll_state,
                div()
                    .p_2()
                    .font_family("Cascadia Mono")
                    .text_size(px(11.0))
                    .text_color(theme::colors().text_primary)
                    .child(String::from_utf8_lossy(&terminal.output).into_owned()),
            )
            .size_full(),
        )
    } else {
        panel_empty_state(
            ">_",
            "Terminal は停止中です",
            "Start で Workspace の PTY を起動します",
        )
    }
}
