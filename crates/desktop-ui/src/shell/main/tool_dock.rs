use super::*;

impl Editor {
    pub(super) fn render_tool_content(&self, cx: &mut Context<Self>) -> gpui::Div {
        match self.shell.active_tool.as_str() {
            id::VIEW_FILES => self.render_files_content(cx),
            id::VIEW_SEARCH => self.render_search_content(cx),
            id::VIEW_GIT => self.render_git_content(cx),
            id::VIEW_HISTORY => div()
                .flex()
                .flex_col()
                .flex_1()
                .p(px(10.0))
                .gap_2()
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(theme::subtle())
                        .child("DOCUMENT HISTORY"),
                )
                .child(
                    div()
                        .p_2()
                        .rounded(px(6.0))
                        .bg(theme::surface())
                        .flex()
                        .flex_col()
                        .gap_1()
                        .text_size(px(12.0))
                        .child(
                            div()
                                .text_color(theme::text())
                                .child(format!("Revision {}", self.session.revision())),
                        )
                        .child(div().text_size(px(11.0)).text_color(theme::subtle()).child(
                            if self.session.has_external_change() {
                                "外部変更を検出しました"
                            } else if self.session.is_dirty() {
                                "未保存の変更があります"
                            } else {
                                "保存済み"
                            },
                        )),
                ),
            _ => panel_empty_state(
                "?",
                "Unknown view",
                self.shell.active_tool.as_str().to_owned(),
            ),
        }
    }
}
