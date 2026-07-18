use super::*;

impl Editor {
    pub(super) fn render_problems_content(&self) -> gpui::Div {
        let mut content = div()
            .id("problems-scroll")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .p_2()
            .gap_1()
            .flex()
            .flex_col();
        if let Some(error) = self.problems.lsp.last_error() {
            content = content.child(
                div()
                    .text_size(px(11.0))
                    .text_color(rgb(0xf0a0a0))
                    .child(error.to_owned()),
            );
        }
        for diagnostic in self.problems.lsp.diagnostics().iter().take(100) {
            content = content.child(div().text_size(px(11.0)).text_color(theme::text()).child(
                format!(
                    "{}:{}:{}  {}",
                    diagnostic.path.display(),
                    diagnostic.range.start.line + 1,
                    diagnostic.range.start.utf16_column + 1,
                    diagnostic.message
                ),
            ));
        }
        if self.problems.lsp.last_error().is_none() && self.problems.lsp.diagnostics().is_empty() {
            content = content.child(panel_empty_state(
                "✓",
                "問題はありません",
                "Rust 文書を開くと診断を開始します",
            ));
        }
        div().flex_1().min_h(px(0.0)).child(content)
    }

    pub(super) fn render_output_content(&self) -> gpui::Div {
        div()
            .flex_1()
            .min_h(px(0.0))
            .p_3()
            .text_size(px(11.0))
            .text_color(theme::muted())
            .child(self.status.clone())
    }
}
