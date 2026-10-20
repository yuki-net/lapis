use super::state::SearchFeature;
use crate::{commands::UiCommand, components::panel_empty_state, theme};
use gpui::{Context, Render, Window, div, prelude::*, px};
impl Render for SearchFeature {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut content = div()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .child(self.query.clone())
            .child(self.message.clone());
        if self.query.is_empty() {
            content = content.child(panel_empty_state(
                "⌕",
                "Search",
                "選択範囲またはクリップボードを Ctrl+Shift+F で検索",
            ));
        }
        for (index, hit) in self.hits.iter().enumerate() {
            let path = self.root.as_ref().map(|root| root.join(&hit.path));
            content = content.child(
                div()
                    .id(("search-hit", index))
                    .p_2()
                    .rounded(px(5.0))
                    .hover(|style| style.bg(theme::colors().button_background_hover))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        if let Some(path) = &path {
                            cx.emit(UiCommand::OpenPath(path.clone()));
                        }
                    }))
                    .child(format!(
                        "{}:{}:{} {}",
                        hit.path.display(),
                        hit.line,
                        hit.utf8_column,
                        hit.preview
                    )),
            );
        }
        content
    }
}
