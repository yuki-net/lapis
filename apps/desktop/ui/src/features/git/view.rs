use super::state::{GitInputKind, GitOperation};
use super::{GitFeature, change_color, change_label};
use crate::{
    components::{action_button, panel_empty_state},
    theme,
};
use gpui::{Context, Render, Window, div, prelude::*, px};
use lapis_git::{ChangeKind, ConflictSide, GitAction};

impl Render for GitFeature {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut content = div()
            .id("git-content")
            .flex()
            .flex_col()
            .p_2()
            .gap_2()
            .child(self.context_label.clone())
            .child(
                div().flex().flex_wrap().gap_1().children(
                    [
                        ("履歴", GitAction::History),
                        ("Branches", GitAction::Branches),
                        ("Fetch", GitAction::Fetch),
                        ("Pull（FF only）", GitAction::Pull),
                        ("Push", GitAction::Push),
                        ("Stash一覧", GitAction::Stashes),
                        ("Merge中止…", GitAction::AbortMerge),
                    ]
                    .into_iter()
                    .map(|(label, action)| {
                        action_button(label, false).on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.execute_action(action.clone(), window, cx)
                            },
                        ))
                    }),
                ),
            )
            .child(
                div().flex().flex_wrap().gap_1().children(
                    [
                        ("Commit…", GitInputKind::Commit),
                        ("Branch作成…", GitInputKind::CreateBranch),
                        ("Branch切替…", GitInputKind::SwitchBranch),
                        ("Branch削除…", GitInputKind::DeleteBranch),
                        ("Merge…", GitInputKind::Merge),
                        ("Stash保存…", GitInputKind::StashPush),
                        ("Stash適用…", GitInputKind::StashApply),
                        ("Stash Pop…", GitInputKind::StashPop),
                        ("Stash削除…", GitInputKind::StashDrop),
                    ]
                    .into_iter()
                    .map(|(label, kind)| {
                        action_button(label, false).on_click(
                            cx.listener(move |this, _, _, cx| this.prompt_input(kind, cx)),
                        )
                    }),
                ),
            )
            .child(
                action_button("登録環境", self.target.is_none())
                    .on_click(cx.listener(|this, _, _, cx| this.select_target(None, cx))),
            )
            .child(action_button("Worktree作成", false).on_click(
                cx.listener(|this, _, _, cx| this.submit(GitOperation::CreateWorktree, cx)),
            ))
            .child(
                action_button("Taskを登録環境へ戻す", false)
                    .on_click(cx.listener(|this, _, _, cx| this.open_task_worktree(None, cx))),
            );
        for (index, (id, status)) in self.snapshot.worktree_statuses.iter().enumerate() {
            let select = id.clone();
            let remove = id.clone();
            let open = id.clone();
            content = content.child(
                div()
                    .id(("git-worktree", index))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        action_button(
                            format!("Worktree {} · {}", id.as_str(), status.branch),
                            self.target.as_ref() == Some(id),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_target(Some(select.clone()), cx)
                        })),
                    )
                    .child(action_button("このTaskで作業", false).on_click(cx.listener(
                        move |this, _, _, cx| this.open_task_worktree(Some(open.clone()), cx),
                    )))
                    .child(action_button("Worktree削除…", false).on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.confirm_operation(
                                GitOperation::Discard(remove.clone()),
                                format!("Worktree {}を削除", remove.as_str()),
                                window,
                                cx,
                            );
                        },
                    ))),
            );
        }
        if let Some(status) = self.selected_status() {
            content = content.child(format!(
                "{} · {} · {} changes",
                status.branch,
                status.head.chars().take(10).collect::<String>(),
                status.files.len()
            ));
            for (index, file) in status.files.iter().enumerate() {
                let path = file.path.clone();
                let stage_path = path.clone();
                let staged = file.staged;
                let worktree = self.target.clone();
                let mut row = div()
                    .id(("git-file", index))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .border_1()
                    .border_color(theme::colors().border_default)
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(change_color(file.kind))
                            .child(format!(
                                "{} · {} · {}",
                                if staged { "INDEX" } else { "WORKING" },
                                change_label(file.kind),
                                path
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_1()
                            .child(action_button("Diff", false).on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.execute_action(
                                        GitAction::Diff {
                                            path: path.clone(),
                                            staged,
                                        },
                                        window,
                                        cx,
                                    );
                                },
                            )))
                            .child(
                                action_button(if staged { "Unstage" } else { "Stage" }, true)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.execute_action(
                                            if staged {
                                                GitAction::Unstage {
                                                    path: stage_path.clone(),
                                                }
                                            } else {
                                                GitAction::Stage {
                                                    path: stage_path.clone(),
                                                }
                                            },
                                            window,
                                            cx,
                                        );
                                    })),
                            ),
                    );
                if file.kind == ChangeKind::Conflicted {
                    let ours = file.path.clone();
                    let theirs = ours.clone();
                    row = row.child(
                        div()
                            .flex()
                            .gap_1()
                            .child(action_button("Oursを採用…", false).on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.execute_action(
                                        GitAction::Resolve {
                                            path: ours.clone(),
                                            side: ConflictSide::Ours,
                                        },
                                        window,
                                        cx,
                                    )
                                },
                            )))
                            .child(action_button("Theirsを採用…", false).on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.execute_action(
                                        GitAction::Resolve {
                                            path: theirs.clone(),
                                            side: ConflictSide::Theirs,
                                        },
                                        window,
                                        cx,
                                    )
                                },
                            )))
                            .child("Editorで修正して保存した後、Stage → Commitでも解消できます。"),
                    );
                }
                if let Some(worktree) = worktree {
                    let path = file.path.clone();
                    row = row.child(
                        action_button("登録環境へ取込…", false).on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.confirm_operation(
                                    GitOperation::Import(worktree.clone(), path.clone()),
                                    format!("Worktree {} の {} を取込", worktree.as_str(), path),
                                    window,
                                    cx,
                                );
                            },
                        )),
                    );
                }
                content = content.child(row);
            }
        } else {
            content = content.child(panel_empty_state(
                "⑂",
                "Git repositoryを確認中です",
                "Project / Taskの操作対象を選択してください",
            ));
        }
        if let (Some(diff), Some(selection)) =
            (&self.snapshot.selected_diff, &self.snapshot.inspection)
        {
            content = content.child(format!(
                "DIFF · {} · +{} -{}",
                diff.path, diff.additions, diff.deletions
            ));
            let target_matches =
                selection.repository_id.is_none() && selection.worktree == self.target;
            let patch = std::sync::Arc::new(diff.patch.clone());
            for (index, _) in diff
                .patch
                .lines()
                .filter(|line| line.starts_with("@@ "))
                .enumerate()
            {
                let selection = selection.clone();
                let patch = patch.clone();
                if target_matches {
                    content = content.child(
                        action_button(
                            format!(
                                "hunk {}を{}",
                                index + 1,
                                if selection.staged { "Unstage" } else { "Stage" }
                            ),
                            false,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.execute_action(
                                    GitAction::StageHunk {
                                        path: selection.path.clone(),
                                        staged: selection.staged,
                                        hunk: index,
                                        expected_patch: patch.as_ref().clone(),
                                    },
                                    window,
                                    cx,
                                )
                            },
                        )),
                    );
                }
            }
            content = content.children(diff.patch.lines().map(|line| {
                div()
                    .font_family("Cascadia Mono")
                    .text_size(px(11.0))
                    .text_color(if line.starts_with('+') {
                        theme::colors().positive_text
                    } else if line.starts_with('-') {
                        theme::colors().danger_text
                    } else {
                        theme::colors().text_secondary
                    })
                    .child(line.to_owned())
            }));
        }
        content = content.children(
            self.snapshot
                .output
                .lines()
                .map(|line| div().text_size(px(11.0)).child(line.to_owned())),
        );
        div()
            .flex()
            .flex_col()
            .min_h(px(0.0))
            .child(self.message.clone())
            .child(content)
    }
}
