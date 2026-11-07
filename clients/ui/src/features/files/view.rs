use super::*;
use crate::{
    components::{FileIcon, Icon, IconName, selectable_row},
    tokens,
};

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
                    .bg(theme::colors(cx).button_background_selected)
                    .text_size(tokens::typography::FONT_SM)
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme::colors(cx).text_primary)
                    .child(workspace_name),
            );

        for (index, entry) in self.visible_entries() {
            let path = entry.path.clone();
            let is_file = entry.kind == FileEntryKind::File;
            let is_selected_file =
                is_file && active_path.map(|p| p == path.as_path()).unwrap_or(false);
            let is_expanded = !is_file && self.expanded_directories.contains(&entry.relative_path);
            let file_info =
                crate::features::files::display_info(&entry.path, entry.kind, &self.languages);
            let label = entry
                .relative_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| entry.relative_path.display().to_string());
            let directory_path = entry.relative_path.clone();
            let file_path = path.clone();

            let row = selectable_row(("workspace-entry", index), is_selected_file, cx)
                .h(px(27.0))
                .flex_shrink_0()
                .pl(px(6.0 + entry.depth as f32 * 16.0))
                .pr(tokens::spacing::XS)
                .flex()
                .items_center()
                .gap(tokens::spacing::XS)
                .text_size(tokens::typography::FONT_SM)
                .text_color(theme::colors(cx).text_primary)
                .child(
                    div()
                        .size(px(16.0))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(is_file, |slot| {
                            slot.child(FileIcon::new(file_info.icon).size(px(16.0)))
                        })
                        .when(!is_file, |slot| {
                            slot.child(
                                Icon::new(if is_expanded {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .size(px(14.0)),
                            )
                        }),
                )
                .child(div().whitespace_nowrap().child(label));

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

    fn visible_entries(&self) -> impl Iterator<Item = (usize, &FileEntry)> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| self.file_entry_visible(entry))
    }

    fn file_entry_visible(&self, entry: &workspace::FileEntry) -> bool {
        if entry.depth == 0 {
            return true;
        }
        let mut relative = entry.relative_path.clone();
        for _ in 0..entry.depth {
            let Some(parent) = relative.parent() else {
                return false;
            };
            if !self.expanded_directories.contains(parent) {
                return false;
            }
            relative = parent.to_owned();
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expansion_uses_workspace_relative_paths_when_host_paths_differ() {
        let mut feature = FilesFeature::new();
        feature.root = Some(".".into());
        feature.entries = vec![
            FileEntry {
                path: "/host/workspace/folder".into(),
                relative_path: "folder".into(),
                kind: FileEntryKind::Directory,
                depth: 0,
            },
            FileEntry {
                path: "/host/workspace/folder/child.txt".into(),
                relative_path: "folder/child.txt".into(),
                kind: FileEntryKind::File,
                depth: 1,
            },
        ];
        assert_eq!(feature.visible_entries().count(), 1);
        feature.expanded_directories.insert("folder".into());
        assert_eq!(feature.visible_entries().count(), 2);
    }

    #[test]
    fn collapsed_children_do_not_hide_later_siblings() {
        let mut feature = FilesFeature::new();
        let root = std::path::PathBuf::from("workspace");
        feature.root = Some(root.clone());
        let make = |relative: std::path::PathBuf, kind, depth| FileEntry {
            path: root.join(&relative),
            relative_path: relative,
            kind,
            depth,
        };
        feature
            .entries
            .push(make("folder".into(), FileEntryKind::Directory, 0));
        feature.entries.extend(
            (0..600)
                .map(|index| make(format!("folder/{index}.txt").into(), FileEntryKind::File, 1)),
        );
        feature
            .entries
            .push(make("later.txt".into(), FileEntryKind::File, 0));
        let visible = feature
            .visible_entries()
            .map(|(_, entry)| entry.relative_path.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            visible,
            vec![
                std::path::PathBuf::from("folder"),
                std::path::PathBuf::from("later.txt")
            ]
        );
        feature.expanded_directories.insert("folder".into());
        assert_eq!(feature.visible_entries().count(), 602);
    }
}
