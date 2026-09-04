use gpui::{div, prelude::*, px};
use lapis_app_services::EditorSession;

use crate::theme;

pub(crate) fn preview_lines(session: &EditorSession) -> Vec<gpui::Div> {
    (0..session.len_lines())
        .filter_map(|line_index| session.line(line_index))
        .map(|line| {
            let line = line.trim_end_matches(['\r', '\n']);
            let (text, size, color) = if let Some(value) = line.strip_prefix("# ") {
                (value.to_owned(), px(24.0), theme::colors().text_primary)
            } else if let Some(value) = line.strip_prefix("## ") {
                (value.to_owned(), px(19.0), theme::colors().text_primary)
            } else if let Some(value) = line.strip_prefix("### ") {
                (value.to_owned(), px(16.0), theme::colors().text_primary)
            } else if let Some(value) = line.strip_prefix("- ") {
                (
                    format!("• {value}"),
                    px(13.0),
                    theme::colors().text_secondary,
                )
            } else if let Some(value) = line.strip_prefix("> ") {
                (format!("│ {value}"), px(13.0), theme::colors().text_accent)
            } else {
                (line.to_owned(), px(13.0), theme::colors().text_secondary)
            };
            div()
                .min_h(px(22.0))
                .text_size(size)
                .text_color(color)
                .child(text)
        })
        .collect()
}
