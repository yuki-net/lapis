use super::ProblemsFeature;
use gpui::{Context, Render, Window, div, prelude::*};
impl Render for ProblemsFeature {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .child(self.message.clone())
            .children(self.state.diagnostics.iter().take(100).map(|diagnostic| {
                div().child(format!(
                    "{}:{}:{} {}",
                    diagnostic.path.display(),
                    diagnostic.range.start.line + 1,
                    diagnostic.range.start.utf16_column + 1,
                    diagnostic.message
                ))
            }))
    }
}
