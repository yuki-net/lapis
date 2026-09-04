use super::*;

impl Editor {
    pub(super) fn render_search_content(&self, cx: &mut Context<Self>) -> gpui::Div {
        if self.search.workspace.is_running() || !self.search.workspace.query().is_empty() {
            let mut content = div()
                .id("workspace-search-scroll")
                .flex()
                .flex_col()
                .flex_1()
                .p_2()
                .gap_1()
                .child(
                    div()
                        .p_2()
                        .rounded(px(5.0))
                        .bg(theme::colors().background_tertiary)
                        .text_size(px(12.0))
                        .child(self.search.workspace.query().to_owned()),
                )
                .child(
                    div()
                        .py_1()
                        .text_size(px(10.0))
                        .text_color(theme::colors().text_tertiary)
                        .child(if self.search.workspace.is_running() {
                            "SEARCHING WORKSPACE…".to_owned()
                        } else {
                            format!(
                                "{} WORKSPACE MATCHES · CTRL SHIFT F",
                                self.search.workspace.hits().len()
                            )
                        }),
                );
            for (index, hit) in self.search.workspace.hits().iter().cloned().enumerate() {
                let path = hit.path.clone();
                content = content.child(
                    div()
                        .id(("workspace-search-hit", index))
                        .px_2()
                        .py_1()
                        .rounded(px(5.0))
                        .flex()
                        .flex_col()
                        .hover(|style| style.bg(theme::colors().button_background_hover))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let Some(root) = this.session.workspace_root().map(ToOwned::to_owned)
                            else {
                                return;
                            };
                            match this.session.open_path(root.join(&path)) {
                                Ok(()) => {
                                    this.restore_active_view();
                                    window.focus(&this.focus_handle);
                                }
                                Err(error) => this.status = format!("読み込み失敗: {error}"),
                            }
                            cx.notify();
                        }))
                        .child(
                            div()
                                .text_size(px(10.0))
                                .text_color(theme::colors().text_tertiary)
                                .child(format!(
                                    "{}:{}:{}",
                                    hit.path.display(),
                                    hit.line,
                                    hit.utf8_column
                                )),
                        )
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(theme::colors().text_secondary)
                                .child(hit.preview),
                        ),
                );
            }
            div().flex().flex_1().min_h(px(0.0)).child(content)
        } else if self.search.query.is_empty() {
            tool_empty_state(
                "⌕",
                "Search",
                "文書内は Ctrl+F、Workspace 全体は Ctrl+Shift+F",
                "選択がない場合はクリップボードの文字列を検索します",
            )
        } else {
            let mut content = div()
                .id("document-search-scroll")
                .flex()
                .flex_col()
                .flex_1()
                .p_2()
                .gap_1()
                .child(
                    div()
                        .p_2()
                        .rounded(px(5.0))
                        .bg(theme::colors().background_tertiary)
                        .text_size(px(12.0))
                        .child(self.search.query.clone()),
                )
                .child(
                    div()
                        .py_1()
                        .text_size(px(10.0))
                        .text_color(theme::colors().text_tertiary)
                        .child(format!("{} MATCHES · F3 NEXT", self.search.matches.len())),
                );
            for (index, range) in self.search.matches.iter().cloned().enumerate() {
                let label = format!("Match {} · {}..{}", index + 1, range.start, range.end);
                content = content.child(
                    div()
                        .id(("search-match", index))
                        .h(px(28.0))
                        .px_2()
                        .rounded(px(5.0))
                        .flex()
                        .items_center()
                        .text_size(px(11.0))
                        .text_color(theme::colors().text_secondary)
                        .bg(if index == self.search.current_match {
                            theme::colors().button_background_selected
                        } else {
                            theme::colors().background_secondary
                        })
                        .hover(|style| style.bg(theme::colors().button_background_hover))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.search.current_match = index;
                            this.selected_range = range.clone();
                            cx.notify();
                        }))
                        .child(label),
                );
            }
            div().flex().flex_1().min_h(px(0.0)).child(content)
        }
    }
}
