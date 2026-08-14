use super::*;

impl Editor {
    pub(super) fn preview_lines(&self) -> Vec<gpui::Div> {
        (0..self.session.len_lines())
            .filter_map(|line_index| self.session.line(line_index))
            .map(|line| {
                let line = line.trim_end_matches(['\r', '\n']);
                let (text, size, color) = if let Some(value) = line.strip_prefix("# ") {
                    (value.to_owned(), px(24.0), theme::text())
                } else if let Some(value) = line.strip_prefix("## ") {
                    (value.to_owned(), px(19.0), theme::text())
                } else if let Some(value) = line.strip_prefix("### ") {
                    (value.to_owned(), px(16.0), theme::text())
                } else if let Some(value) = line.strip_prefix("- ") {
                    (format!("• {value}"), px(13.0), theme::muted())
                } else if let Some(value) = line.strip_prefix("> ") {
                    (format!("│ {value}"), px(13.0), theme::assistant_accent())
                } else {
                    (line.to_owned(), px(13.0), theme::muted())
                };
                div()
                    .min_h(px(22.0))
                    .text_size(size)
                    .text_color(color)
                    .child(text)
            })
            .collect()
    }
}
