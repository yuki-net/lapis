use super::*;
use crate::tokens;

impl Editor {
    pub(super) fn render_files_content(&self, cx: &mut Context<Self>) -> gpui::Div {
        let workspace_name = self.session.workspace_name().to_owned();

        let active_path = self.session.active_path();

        let mut content = div()
            .id("files-scroll")
            .flex()
            .flex_col()
            .flex_1()
            .p(tokens::spacing::XS)
            .child(
                div()
                    .h(px(30.0))
                    .px(tokens::spacing::XS)
                    .rounded(tokens::radius::CONTROL)
                    .flex()
                    .items_center()
                    .bg(theme::colors().button_background_selected)
                    .text_size(tokens::typography::FONT_SM)
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme::colors().text_primary)
                    .child(workspace_name),
            );

        for (index, entry) in self.session.file_tree().iter().enumerate() {
            if index >= 500 || !self.file_entry_visible(entry) {
                continue;
            }

            let path = entry.path.clone();
            let is_file = entry.kind == FileEntryKind::File;
            let is_selected_file =
                is_file && active_path.map(|p| p == path.as_path()).unwrap_or(false);
            let is_expanded = !is_file && self.expanded_directories.contains(&path);
            let has_children = !is_file
                && self
                    .session
                    .file_tree()
                    .iter()
                    .any(|candidate| candidate.path.parent() == Some(path.as_path()));
            let file_info = crate::features::files::display_info(
                &entry.path,
                entry.kind,
                &self.problems.languages,
            );
            let label = entry
                .relative_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("?")
                .to_owned();
            let directory_path = path.clone();
            let file_path = path.clone();

            let row_bg = if is_selected_file {
                theme::colors().button_background_selected
            } else {
                gpui::rgba(0x00000000)
            };

            let row = div()
                .id(("workspace-entry", index))
                .h(px(27.0))
                .pl(px(6.0 + entry.depth as f32 * 14.0))
                .pr(tokens::spacing::XS)
                .rounded(tokens::radius::CONTROL)
                .bg(row_bg)
                .flex()
                .items_center()
                .gap(tokens::spacing::XS)
                .text_size(tokens::typography::FONT_SM)
                .text_color(theme::colors().text_primary)
                .hover(|style| {
                    if !is_selected_file {
                        style.bg(theme::colors().button_background_hover)
                    } else {
                        style
                    }
                })
                .child(
                    div()
                        .w(px(14.0))
                        .flex_shrink_0()
                        .text_color(theme::colors().text_secondary)
                        .child(if is_file || !has_children {
                            ""
                        } else if is_expanded {
                            "⌄"
                        } else {
                            ">"
                        }),
                )
                .child(crate::components::FileIcon::new(file_info.icon))
                .child(label);

            content = if is_file {
                content.child(row.on_click(cx.listener(move |this, _, window, cx| {
                    this.shell.focused_panel = crate::extension_ui::PanelPosition::Left;
                    match this.session.open_path(file_path.clone()) {
                        Ok(()) => {
                            this.restore_active_view();
                            this.shell.synchronize_documents(&this.session.tabs());
                            this.status = "文書を開きました".to_owned();
                            window.focus(&this.focus_handle);
                            this.refresh_feature_activation();
                        }
                        Err(error) => {
                            this.status = format!("読み込み失敗: {error}");
                        }
                    }
                    cx.notify();
                })))
            } else {
                content.child(row.on_click(cx.listener(move |this, _, _, cx| {
                    this.shell.focused_panel = crate::extension_ui::PanelPosition::Left;
                    if !this.expanded_directories.remove(&directory_path) {
                        this.expanded_directories.insert(directory_path.clone());
                    }
                    cx.notify();
                })))
            };
        }

        div().flex().flex_1().min_h(px(0.0)).child(content)
    }

    fn file_entry_visible(&self, entry: &lapis_workspace::FileEntry) -> bool {
        if entry.depth == 0 {
            return true;
        }
        let Some(root) = self.session.workspace_root() else {
            return false;
        };
        let mut relative = entry.relative_path.clone();
        for _ in 0..entry.depth {
            let Some(parent) = relative.parent() else {
                return false;
            };
            if !self.expanded_directories.contains(&root.join(parent)) {
                return false;
            }
            relative = parent.to_owned();
        }
        true
    }
}
