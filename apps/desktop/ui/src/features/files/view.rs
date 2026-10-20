use super::*;
use crate::tokens;

impl FilesFeature {
    pub(super) fn render_files_content(&self, cx: &mut Context<Self>) -> gpui::Div {
        let workspace_name = self.workspace_name.clone();

        let active_path = self.active_path.as_deref();

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

        for (index, entry) in self.entries.iter().enumerate() {
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
                    .entries
                    .iter()
                    .any(|candidate| candidate.path.parent() == Some(path.as_path()));
            let file_info =
                crate::features::files::display_info(&entry.path, entry.kind, &self.languages);
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
                content.child(row.on_click(cx.listener(move |_, _, window, cx| {
                    cx.emit(UiCommand::OpenPath(file_path.clone()));
                    let _ = window;
                    cx.notify();
                })))
            } else {
                content.child(row.on_click(cx.listener(move |this, _, _, cx| {
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
        let Some(root) = self.root.as_deref() else {
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
