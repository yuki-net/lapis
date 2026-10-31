use super::state::GitOperation;
use super::{GitFeature, change_color, change_label};
use crate::{
    components::{action_button, panel_empty_state},
    theme,
};
use gpui::{Context, Entity, Render, Subscription, Window, div, prelude::*, px};
use lapis_git::GitAction;
use std::collections::{BTreeMap, HashSet};

#[derive(Default)]
struct FileTree {
    children: BTreeMap<String, FileTree>,
    files: Vec<lapis_git::GitFileSnapshot>,
}

enum FileRow {
    Directory(String, usize),
    File(lapis_git::GitFileSnapshot, usize),
}

fn file_rows(files: Vec<lapis_git::GitFileSnapshot>, tree: bool) -> Vec<FileRow> {
    if !tree {
        return files
            .into_iter()
            .map(|file| FileRow::File(file, 0))
            .collect();
    }
    let mut root = FileTree::default();
    for file in files {
        let mut node = &mut root;
        let components = file.path.split('/').collect::<Vec<_>>();
        for component in &components[..components.len().saturating_sub(1)] {
            node = node.children.entry((*component).to_owned()).or_default();
        }
        node.files.push(file);
    }
    fn append(node: FileTree, depth: usize, rows: &mut Vec<FileRow>) {
        for file in node.files {
            rows.push(FileRow::File(file, depth));
        }
        for (mut label, mut child) in node.children {
            while child.files.is_empty() && child.children.len() == 1 {
                let (name, next) = child.children.into_iter().next().unwrap();
                label.push('/');
                label.push_str(&name);
                child = next;
            }
            rows.push(FileRow::Directory(label, depth));
            append(child, depth + 1, rows);
        }
    }
    let mut rows = Vec::new();
    append(root, 0, &mut rows);
    rows
}

pub(crate) struct ChangesFeature {
    git: Entity<GitFeature>,
    collapsed: HashSet<String>,
    tree: bool,
    _subscription: Subscription,
}

impl ChangesFeature {
    pub(crate) fn new(git: Entity<GitFeature>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&git, |_, _, cx| cx.notify());
        Self {
            git,
            collapsed: HashSet::new(),
            tree: true,
            _subscription: subscription,
        }
    }
}

impl Render for ChangesFeature {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let git = self.git.read(cx);
        let repositories = git.snapshot.repositories.clone();
        let repository_errors = git.snapshot.repository_errors.clone();
        let last_refresh_error = git.snapshot.last_refresh_error.clone();
        let message = git.message.clone();
        let diff = git.snapshot.selected_diff.clone();
        let selection = git.snapshot.inspection.clone();
        let can_restore = selection.as_ref().is_some_and(|selection| {
            !selection.staged
                && selection.repository_id.as_deref().is_some_and(|id| {
                    repositories
                        .iter()
                        .find(|(repository, _)| repository.id == id)
                        .is_some_and(|(_, status)| {
                            status.files.iter().any(|file| {
                                file.path == selection.path
                                    && !file.staged
                                    && !matches!(
                                        file.kind,
                                        lapis_git::ChangeKind::Untracked
                                            | lapis_git::ChangeKind::Conflicted
                                    )
                            })
                        })
                })
        });
        let mut content = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .child(
                action_button("Rescan", false, cx).on_click(cx.listener(|this, _, _, cx| {
                    this.git.update(cx, |git, cx| {
                        git.submit(GitOperation::RescanRepositories, cx)
                    });
                })),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(action_button("Tree", self.tree, cx).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.tree = true;
                            cx.notify();
                        },
                    )))
                    .child(action_button("List", !self.tree, cx).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.tree = false;
                            cx.notify();
                        },
                    ))),
            )
            .child(message);
        if let Some(error) = last_refresh_error {
            content = content.child(
                div()
                    .text_size(px(11.))
                    .text_color(theme::colors(cx).text_secondary)
                    .child(format!("Git状態の更新に失敗しました: {error}")),
            );
        }
        if repositories.is_empty() && repository_errors.is_empty() {
            content = content.child(panel_empty_state(
                "⑂",
                "Repository",
                "Open or rescan a project",
                cx,
            ));
        }
        for (index, (repository, status)) in repositories.into_iter().enumerate() {
            let id = repository.id.clone();
            let collapsed = self.collapsed.contains(&id);
            let label = repository.path.unwrap_or_else(|| ".".into());
            let toggle_id = id.clone();
            let commit_id = id.clone();
            let amend_id = id.clone();
            let repository_error = repository_errors
                .iter()
                .find(|(failed, _)| failed.id == id)
                .map(|(_, error)| error.clone());
            let mut group = div()
                .id(("changes-repository", index))
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_1()
                        .child(
                            action_button(
                                format!(
                                    "{} {}  ·  {}  ·  {}",
                                    if collapsed { ">" } else { "v" },
                                    label,
                                    status.branch,
                                    status.files.len()
                                ),
                                false,
                                cx,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if !this.collapsed.insert(toggle_id.clone()) {
                                        this.collapsed.remove(&toggle_id);
                                    }
                                    cx.notify();
                                },
                            )),
                        )
                        .child(action_button("Commit...", false, cx).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.git.update(cx, |git, cx| {
                                    git.prompt_repository_commit(commit_id.clone(), false, cx)
                                });
                            },
                        )))
                        .child(action_button("Amend...", false, cx).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.git.update(cx, |git, cx| {
                                    git.prompt_repository_commit(amend_id.clone(), true, cx)
                                });
                            },
                        ))),
                );
            if let Some(error) = repository_error {
                group = group.child(
                    div()
                        .text_size(px(11.))
                        .text_color(theme::colors(cx).text_secondary)
                        .child(format!("Status stale: {error}")),
                );
            }
            if !collapsed {
                for (file_index, row) in file_rows(status.files, self.tree).into_iter().enumerate()
                {
                    let (file, depth) = match row {
                        FileRow::Directory(name, depth) => {
                            group = group.child(div().pl(px((depth * 12) as f32)).child(name));
                            continue;
                        }
                        FileRow::File(file, depth) => (file, depth),
                    };
                    let diff_id = id.clone();
                    let stage_id = id.clone();
                    let diff_path = file.path.clone();
                    let stage_path = file.path.clone();
                    let staged = file.staged;
                    group = group.child(
                        div()
                            .id(gpui::SharedString::from(format!(
                                "changes-file-{index}-{file_index}"
                            )))
                            .flex()
                            .flex_wrap()
                            .gap_1()
                            .pl(px((depth * 12) as f32))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(change_color(file.kind, cx))
                                    .child(format!(
                                        "{} {} {}",
                                        if file.kind == lapis_git::ChangeKind::Conflicted {
                                            "CONFLICT"
                                        } else if staged {
                                            "INDEX"
                                        } else {
                                            "WORKING"
                                        },
                                        change_label(file.kind),
                                        if self.tree {
                                            file.path.rsplit('/').next().unwrap_or(&file.path)
                                        } else {
                                            &file.path
                                        }
                                    )),
                            )
                            .child(action_button("Diff", false, cx).on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.git.update(cx, |git, cx| {
                                        git.execute_repository_action(
                                            diff_id.clone(),
                                            GitAction::Diff {
                                                path: diff_path.clone(),
                                                staged,
                                            },
                                            window,
                                            cx,
                                        )
                                    });
                                },
                            )))
                            .child(
                                action_button(if staged { "Unstage" } else { "Stage" }, false, cx)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        let action = if staged {
                                            GitAction::Unstage {
                                                path: stage_path.clone(),
                                            }
                                        } else {
                                            GitAction::Stage {
                                                path: stage_path.clone(),
                                            }
                                        };
                                        this.git.update(cx, |git, cx| {
                                            git.execute_repository_action(
                                                stage_id.clone(),
                                                action,
                                                window,
                                                cx,
                                            )
                                        });
                                    })),
                            ),
                    );
                }
            }
            content = content.child(group);
        }
        for (index, (repository, error)) in repository_errors
            .into_iter()
            .filter(|(failed, _)| {
                !git.snapshot
                    .repositories
                    .iter()
                    .any(|(repository, _)| repository.id == failed.id)
            })
            .enumerate()
        {
            let label = repository.path.unwrap_or_else(|| ".".into());
            content = content.child(
                div()
                    .id(("changes-repository-error", index))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(format!("{label} · Status unavailable"))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme::colors(cx).text_secondary)
                            .child(error),
                    ),
            );
        }
        if let (Some(diff), Some(selection)) = (diff, selection)
            && let Some(repository_id) = selection.repository_id
        {
            content = content.child(format!(
                "DIFF · {} · {} · +{} -{}",
                repository_id, diff.path, diff.additions, diff.deletions
            ));
            if can_restore && !diff.patch.is_empty() {
                let restore_id = repository_id.clone();
                let restore_path = selection.path.clone();
                let restore_patch = diff.patch.clone();
                content = content.child(action_button("Restore...", false, cx).on_click(
                    cx.listener(move |this, _, window, cx| {
                        this.git.update(cx, |git, cx| {
                            git.execute_repository_action(
                                restore_id.clone(),
                                GitAction::Restore {
                                    path: restore_path.clone(),
                                    expected_patch: restore_patch.clone(),
                                },
                                window,
                                cx,
                            )
                        });
                    }),
                ));
            }
            content = content.children(diff.patch.lines().map(|line| {
                div()
                    .font_family("Cascadia Mono")
                    .text_size(px(11.))
                    .text_color(theme::colors(cx).text_secondary)
                    .child(line.to_owned())
            }));
        }
        div().flex().flex_col().min_h(px(0.)).child(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lapis_git::{ChangeKind, GitFileSnapshot};
    #[test]
    fn tree_compacts_single_child_directories_and_preserves_files() {
        let file = |path: &str| GitFileSnapshot {
            path: path.into(),
            old_path: None,
            kind: ChangeKind::Modified,
            staged: false,
        };
        let rows = file_rows(
            vec![
                file("apps/desktop/ui/main.rs"),
                file("apps/desktop/ui/lib.rs"),
            ],
            true,
        );
        assert!(matches!(&rows[0], FileRow::Directory(name, 0) if name == "apps/desktop/ui"));
        assert_eq!(
            rows.iter()
                .filter(|row| matches!(row, FileRow::File(_, 1)))
                .count(),
            2
        );
        assert_eq!(file_rows(vec![file("a/b.rs")], false).len(), 1);
    }
}
