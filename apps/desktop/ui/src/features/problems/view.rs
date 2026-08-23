use crate::{
    components::{ScrollAxis, ScrollableElement, panel_empty_state},
    features::problems::ProblemsFeature,
    theme,
};
use gpui::{div, prelude::*, px};

pub(crate) fn render_content(problems: &ProblemsFeature) -> gpui::Div {
    let mut content = div()
        .id("problems-scroll")
        .flex_1()
        .min_h(px(0.0))
        .scrollable(ScrollAxis::Vertical)
        .p_2()
        .gap_1()
        .flex()
        .flex_col();
    if let Some(error) = problems.lsp.last_error() {
        content = content.child(
            div()
                .text_size(px(11.0))
                .text_color(theme::colors().danger_text)
                .child(error.to_owned()),
        );
    }
    for diagnostic in problems.lsp.diagnostics().iter().take(100) {
        content = content.child(
            div()
                .text_size(px(11.0))
                .text_color(theme::colors().text_primary)
                .child(format!(
                    "{}:{}:{}  {}",
                    diagnostic.path.display(),
                    diagnostic.range.start.line + 1,
                    diagnostic.range.start.utf16_column + 1,
                    diagnostic.message
                )),
        );
    }
    if problems.lsp.last_error().is_none() && problems.lsp.diagnostics().is_empty() {
        content = content.child(panel_empty_state(
            "✓",
            "問題はありません",
            "Rust 文書を開くと診断を開始します",
        ));
    }
    div().flex_1().min_h(px(0.0)).child(content)
}

pub(crate) fn render_output(status: &str) -> gpui::Div {
    div()
        .flex_1()
        .min_h(px(0.0))
        .p_3()
        .text_size(px(11.0))
        .text_color(theme::colors().text_secondary)
        .child(status.to_owned())
}
