use super::*;

impl Editor {
    pub(super) fn render_files_content(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut content = div()
            .id("files-scroll")
            .flex()
            .flex_col()
            .flex_1()
            .overflow_y_scroll()
            .p(px(6.0))
            .child(
                div()
                    .h(px(28.0))
                    .px_2()
                    .rounded(px(5.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .bg(theme::surface_active())
                    .text_size(px(12.0))
                    .text_color(theme::text())
                    .child("⌄")
                    .child(self.session.workspace_name().to_owned())
                    .child(div().flex_1())
                    .child("Open…"),
            )
            .child(
                div()
                    .h(px(25.0))
                    .px_2()
                    .flex()
                    .items_center()
                    .text_size(px(10.0))
                    .text_color(theme::subtle())
                    .child("OPEN DOCUMENTS"),
            );
        for (tab_index, tab) in self.session.tabs().into_iter().enumerate() {
            let id = tab.id.clone();
            content = content.child(
                div()
                    .id(("open-document", tab_index))
                    .h(px(27.0))
                    .px_2()
                    .rounded(px(5.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .bg(if tab.active {
                        theme::surface()
                    } else {
                        theme::island()
                    })
                    .text_size(px(12.0))
                    .text_color(theme::text())
                    .hover(|style| style.bg(theme::surface_hover()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.persist_active_view();
                        if this.session.activate_document(&id) {
                            this.restore_active_view();
                            window.focus(&this.focus_handle);
                            cx.notify();
                        }
                    }))
                    .child(file_badge("F", theme::orange()))
                    .child(tab.display_name)
                    .child(div().flex_1())
                    .child(if tab.dirty { "●" } else { "" }),
            );
        }
        content = content.child(
            div()
                .h(px(25.0))
                .px_2()
                .flex()
                .items_center()
                .text_size(px(10.0))
                .text_color(theme::subtle())
                .child("WORKSPACE FILES"),
        );
        for (index, entry) in self.session.file_tree().iter().take(500).enumerate() {
            let path = entry.path.clone();
            let is_file = entry.kind == FileEntryKind::File;
            let label = entry
                .relative_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("?")
                .to_owned();
            content = content.child(
                div()
                    .id(("workspace-entry", index))
                    .h(px(25.0))
                    .pl(px(8.0 + entry.depth as f32 * 12.0))
                    .pr_2()
                    .rounded(px(4.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(px(11.0))
                    .text_color(if is_file {
                        theme::muted()
                    } else {
                        theme::text()
                    })
                    .hover(|style| style.bg(theme::surface_hover()))
                    .when(is_file, |row| {
                        row.on_click(cx.listener(move |this, _, window, cx| {
                            match this.session.open_path(path.clone()) {
                                Ok(()) => {
                                    this.restore_active_view();
                                    this.status = "文書を開きました".to_owned();
                                    window.focus(&this.focus_handle);
                                }
                                Err(error) => {
                                    this.status = format!("読み込み失敗: {error}");
                                }
                            }
                            cx.notify();
                        }))
                    })
                    .child(if is_file { "·" } else { "⌄" })
                    .child(label),
            );
        }
        div().flex().flex_1().min_h(px(0.0)).child(content)
    }
}
