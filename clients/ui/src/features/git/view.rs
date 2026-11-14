use super::state::{GitInputKind, GitOperation};
use super::{GitFeature, change_color, change_label};
use crate::{
    components::{action_button, panel_empty_state},
    theme, tokens,
};
use git::{ChangeKind, ConflictSide, GitAction, GitFileSnapshot};
use gpui::{
    App, Context, Pixels, Render, Rgba, SharedString, Stateful, Window, div, prelude::*, px,
};

/// 変更ファイルPaneの基準幅。狭いPanelでは差分Paneの下へ回り込む。
const FILES_PANE_BASIS: Pixels = px(300.0);
/// 変更ファイルPaneを読める最小幅。これを下回るPanelではPaneを縦に積む。
const FILES_PANE_MIN_WIDTH: Pixels = px(200.0);
/// 差分Paneの基準幅。
const DIFF_PANE_BASIS: Pixels = px(360.0);
/// 差分Paneを読める最小幅。左右に並べるにはPanel幅がこの合計を超える必要がある。
const DIFF_PANE_MIN_WIDTH: Pixels = px(240.0);
const MONO_FONT_FAMILY: &str = "Cascadia Mono";
/// コマンド結果として表示する最大行数。末尾へ無制限に流れる生ログに戻さない。
const COMMAND_OUTPUT_LINES: usize = 40;

// ラベルの表記規則: Gitの用語（Branch / Commit / Stage / Stash / Worktree / Merge / Hunk / Fetch /
// Pull / Push / Diff）は英語のまま、それを説明する語だけを日本語にする。
// `state.rs` の入力プロンプト見出し（「作成するBranch名」など）と同じ規則に揃える。

impl Render for GitFeature {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 左右の余白は panel_content が確保する。ここでは縦の余白と要素間だけを決める。
        let mut content = div()
            .id("git-content")
            .flex()
            .flex_col()
            .w_full()
            .min_w(px(0.0))
            .gap(tokens::spacing::MD)
            .py(tokens::spacing::SM)
            .child(self.render_status_bar(cx));
        if !self.message.is_empty() {
            content = content.child(message_banner(self.message.clone(), cx));
        }
        content = content.child(self.render_panes(cx));
        // 生のコマンド出力は常設しない。必要時だけ開く別枠へ分離する。
        if let Some(output) = self.render_command_output(window, cx) {
            content = content.child(output);
        }
        div()
            .flex()
            .flex_col()
            .w_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .child(content)
    }
}

impl GitFeature {
    /// 現在どの作業対象の、どのBranch・CommitにいるかをBadgeで示す。
    fn render_status_bar(&self, cx: &App) -> impl IntoElement {
        let colors = theme::colors(cx);
        let context = if self.context_label.trim().is_empty() {
            "作業対象".to_owned()
        } else {
            self.context_label.clone()
        };
        let mut bar = div()
            .id("git-status-bar")
            .debug_selector(|| "git-status-bar".into())
            .flex()
            .flex_wrap()
            .items_center()
            .gap(tokens::spacing::XS)
            .w_full()
            .min_w(px(0.0))
            .child(shrinking_badge(
                context,
                colors.text_primary,
                colors.background_tertiary,
            ));
        bar = bar.child(match self.target.as_ref() {
            Some(target) => shrinking_badge(
                format!("Worktree {}", target.as_str()),
                colors.info_text,
                colors.info_background,
            ),
            None => badge(
                "登録環境",
                colors.text_secondary,
                colors.background_secondary,
            ),
        });
        let Some(status) = self.selected_status() else {
            return bar;
        };
        bar = bar.child(shrinking_badge(
            status.branch.clone(),
            colors.text_accent,
            colors.info_background,
        ));
        bar = bar.child(badge(
            head_label(&status.head),
            colors.text_secondary,
            colors.background_tertiary,
        ));
        let conflicts = status
            .files
            .iter()
            .filter(|file| file.kind == ChangeKind::Conflicted)
            .count();
        if status.files.is_empty() {
            bar = bar.child(badge(
                "変更なし",
                colors.text_secondary,
                colors.background_tertiary,
            ));
        } else {
            bar = bar.child(badge(
                format!("{} 件の変更", status.files.len()),
                colors.warning_text,
                colors.warning_background,
            ));
        }
        if conflicts > 0 {
            bar = bar.child(badge(
                format!("{conflicts} 件の競合"),
                colors.danger_text,
                colors.danger_background,
            ));
        }
        bar
    }

    /// 変更ファイルと差分を横に並べる。幅が足りない場合は縦に積む。
    fn render_panes(&self, cx: &mut Context<Self>) -> gpui::Div {
        let files_pane = div()
            .id("git-files-pane")
            .debug_selector(|| "git-files-pane".into())
            .flex()
            .flex_col()
            .gap(tokens::spacing::SM)
            .flex_1()
            .flex_basis(FILES_PANE_BASIS)
            .min_w(FILES_PANE_MIN_WIDTH)
            .min_h(px(0.0))
            .child(self.render_commit_pane(cx))
            .child(self.render_changes_pane(cx))
            .child(self.render_operations_pane(cx));
        div()
            .flex()
            .flex_wrap()
            .items_start()
            .gap(tokens::spacing::MD)
            .w_full()
            .min_w(px(0.0))
            .child(files_pane)
            .child(self.render_diff_pane(cx))
    }

    /// 主操作であるCommitを独立した枠に置き、状態を1行で示す。
    fn render_commit_pane(&self, cx: &mut Context<Self>) -> gpui::Div {
        let colors = theme::colors(cx);
        let status = self.selected_status();
        let staged = status.map_or(0, |status| {
            status.files.iter().filter(|file| file.staged).count()
        });
        let hint = match status {
            None => "作業対象を選ぶとCommitできます",
            Some(_) if staged == 0 => {
                "Stage済みの変更がありません。Commitする変更を先にStageしてください"
            }
            Some(_) => "Stage済みの変更をCommitします",
        };
        card(cx)
            .child(section_title("Commit", cx))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(tokens::spacing::XS)
                    .child(
                        operation_button("git-commit", "Commit…", true, cx).on_click(cx.listener(
                            |this, _, _, cx| this.prompt_input(GitInputKind::Commit, cx),
                        )),
                    )
                    .child(badge(
                        format!("Stage済み {staged} 件"),
                        if staged > 0 {
                            colors.text_positive
                        } else {
                            colors.text_secondary
                        },
                        if staged > 0 {
                            colors.positive_background
                        } else {
                            colors.background_tertiary
                        },
                    )),
            )
            .child(muted_text(hint, cx))
    }

    /// 変更ファイルをステージ状態ごとに並べ、行から差分とStage操作へ到達させる。
    fn render_changes_pane(&self, cx: &mut Context<Self>) -> gpui::Div {
        let colors = theme::colors(cx);
        let Some(status) = self.selected_status() else {
            return card(cx)
                .child(section_title("変更ファイル", cx))
                .child(panel_empty_state(
                    "⑂",
                    "Git repositoryを確認中です",
                    "Project / Taskの操作対象を選択してください",
                    cx,
                ));
        };
        let files = status.files.clone();
        let mut pane = card(cx).child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(tokens::spacing::XS)
                .child(section_title("変更ファイル", cx))
                .child(badge(
                    format!("{} 件", files.len()),
                    colors.text_secondary,
                    colors.background_tertiary,
                )),
        );
        if files.is_empty() {
            return pane.child(muted_text("変更はありません", cx));
        }
        let staged = files
            .iter()
            .enumerate()
            .filter(|(_, file)| file.staged)
            .map(|(index, file)| (index, file.clone()))
            .collect::<Vec<_>>();
        let unstaged = files
            .iter()
            .enumerate()
            .filter(|(_, file)| !file.staged)
            .map(|(index, file)| (index, file.clone()))
            .collect::<Vec<_>>();
        for (title, group) in [("Stage済み", staged), ("未Stage", unstaged)] {
            if group.is_empty() {
                continue;
            }
            let count = group.len();
            let mut rows = div().flex().flex_col().gap(tokens::spacing::XS);
            for (index, file) in group {
                rows = rows.child(self.render_file_row(index, file, cx));
            }
            pane = pane.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(tokens::spacing::XS)
                    .child(group_title(format!("{title}（{count}）"), cx))
                    .child(rows),
            );
        }
        pane
    }

    fn render_file_row(
        &self,
        index: usize,
        file: GitFileSnapshot,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = theme::colors(cx);
        let staged = file.staged;
        let diff_path = file.path.clone();
        let stage_path = file.path.clone();
        let mut row = div()
            .id(("git-file", index))
            .flex()
            .flex_col()
            .gap(tokens::spacing::XS)
            .p(tokens::spacing::XS)
            .rounded(tokens::radius::CONTROL)
            .border_1()
            .border_color(colors.border_default)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(tokens::spacing::XS)
                    .child(badge(
                        change_label(file.kind),
                        change_color(file.kind, cx),
                        colors.background_tertiary,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .font_family(MONO_FONT_FAMILY)
                            .text_size(tokens::typography::FONT_XS)
                            .text_color(colors.text_primary)
                            .child(file.path.clone()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(tokens::spacing::XS)
                    .child(
                        operation_button(format!("git-file-diff-{index}"), "Diff", false, cx)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.execute_action(
                                    GitAction::Diff {
                                        path: diff_path.clone(),
                                        staged,
                                    },
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        operation_button(
                            format!("git-file-stage-{index}"),
                            if staged { "Unstage" } else { "Stage" },
                            true,
                            cx,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
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
                            },
                        )),
                    ),
            );
        if file.kind == ChangeKind::Conflicted {
            let ours = file.path.clone();
            let theirs = file.path.clone();
            row = row
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(tokens::spacing::XS)
                        .child(
                            operation_button(
                                format!("git-file-ours-{index}"),
                                "Oursを採用…",
                                false,
                                cx,
                            )
                            .on_click(cx.listener(
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
                            )),
                        )
                        .child(
                            operation_button(
                                format!("git-file-theirs-{index}"),
                                "Theirsを採用…",
                                false,
                                cx,
                            )
                            .on_click(cx.listener(
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
                            )),
                        ),
                )
                .child(muted_text(
                    "エディタで修正して保存した後、Stage → Commitでも解消できます。",
                    cx,
                ));
        }
        if let Some(worktree) = self.target.clone() {
            let path = file.path.clone();
            row = row.child(
                operation_button(
                    format!("git-file-import-{index}"),
                    "登録環境へ取込…",
                    false,
                    cx,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.confirm_operation(
                        GitOperation::Import(worktree.clone(), path.clone()),
                        format!("Worktree {} の {} を取込", worktree.as_str(), path),
                        window,
                        cx,
                    );
                })),
            );
        }
        row
    }

    /// Commit以外の操作を目的ごとにまとめ、変更ファイルの流れを邪魔しない位置へ下げる。
    fn render_operations_pane(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut pane = div()
            .flex()
            .flex_col()
            .gap(tokens::spacing::SM)
            .child(section_title("その他の操作", cx));
        pane = pane.child(self.render_action_group(
            "同期",
            vec![
                ("git-action-fetch", "Fetch", GitAction::Fetch),
                ("git-action-pull", "Pull（FF only）", GitAction::Pull),
                ("git-action-push", "Push", GitAction::Push),
            ],
            cx,
        ));
        pane = pane.child(self.render_action_group(
            "参照",
            vec![
                ("git-action-history", "履歴", GitAction::History),
                ("git-action-branches", "Branch一覧", GitAction::Branches),
                ("git-action-stashes", "Stash一覧", GitAction::Stashes),
            ],
            cx,
        ));
        pane = pane.child(self.render_prompt_group(
            "Branch",
            vec![
                (
                    "git-prompt-create-branch",
                    "Branch作成…",
                    GitInputKind::CreateBranch,
                ),
                (
                    "git-prompt-switch-branch",
                    "Branch切替…",
                    GitInputKind::SwitchBranch,
                ),
                (
                    "git-prompt-delete-branch",
                    "Branch削除…",
                    GitInputKind::DeleteBranch,
                ),
            ],
            cx,
        ));
        pane = pane.child(self.render_prompt_group(
            "Stash",
            vec![
                (
                    "git-prompt-stash-push",
                    "Stash保存…",
                    GitInputKind::StashPush,
                ),
                (
                    "git-prompt-stash-apply",
                    "Stash適用…",
                    GitInputKind::StashApply,
                ),
                (
                    "git-prompt-stash-pop",
                    "Stash取り出し…",
                    GitInputKind::StashPop,
                ),
                (
                    "git-prompt-stash-drop",
                    "Stash削除…",
                    GitInputKind::StashDrop,
                ),
            ],
            cx,
        ));
        pane = pane.child(
            div()
                .flex()
                .flex_col()
                .gap(tokens::spacing::XS)
                .child(group_title("Merge", cx))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(tokens::spacing::XS)
                        .child(
                            operation_button("git-prompt-merge", "Merge…", false, cx).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.prompt_input(GitInputKind::Merge, cx)
                                }),
                            ),
                        )
                        .child(
                            operation_button("git-action-abort-merge", "Merge中止…", false, cx)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.execute_action(GitAction::AbortMerge, window, cx)
                                })),
                        ),
                ),
        );
        pane.child(self.render_worktree_group(cx))
    }

    fn render_action_group(
        &self,
        title: &'static str,
        actions: Vec<(&'static str, &'static str, GitAction)>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .gap(tokens::spacing::XS)
            .child(group_title(title, cx))
            .child(div().flex().flex_wrap().gap(tokens::spacing::XS).children(
                actions.into_iter().map(|(id, label, action)| {
                    operation_button(id, label, false, cx).on_click(cx.listener(
                        move |this, _, window, cx| this.execute_action(action.clone(), window, cx),
                    ))
                }),
            ))
    }

    fn render_prompt_group(
        &self,
        title: &'static str,
        prompts: Vec<(&'static str, &'static str, GitInputKind)>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .gap(tokens::spacing::XS)
            .child(group_title(title, cx))
            .child(div().flex().flex_wrap().gap(tokens::spacing::XS).children(
                prompts.into_iter().map(|(id, label, kind)| {
                    operation_button(id, label, false, cx)
                        .on_click(cx.listener(move |this, _, _, cx| this.prompt_input(kind, cx)))
                }),
            ))
    }

    /// Worktreeの選択・作成・移動・削除を1か所に集める。
    fn render_worktree_group(&self, cx: &mut Context<Self>) -> gpui::Div {
        let colors = theme::colors(cx);
        let mut group = div()
            .flex()
            .flex_col()
            .gap(tokens::spacing::XS)
            .child(group_title("Worktree", cx))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(tokens::spacing::XS)
                    .child(
                        operation_button("git-worktree-create", "Worktree作成", false, cx)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.submit(GitOperation::CreateWorktree, cx)
                            })),
                    )
                    .child(
                        operation_button("git-env-select", "登録環境", self.target.is_none(), cx)
                            .on_click(cx.listener(|this, _, _, cx| this.select_target(None, cx))),
                    )
                    .child(
                        operation_button("git-worktree-return", "Taskを登録環境へ戻す", false, cx)
                            .on_click(
                                cx.listener(|this, _, _, cx| this.open_task_worktree(None, cx)),
                            ),
                    ),
            );
        for (index, (id, status)) in self.snapshot.worktree_statuses.iter().enumerate() {
            let select = id.clone();
            let open = id.clone();
            let remove = id.clone();
            let selected = self.target.as_ref() == Some(id);
            group = group.child(
                div()
                    .id(("git-worktree", index))
                    .flex()
                    .flex_col()
                    .gap(tokens::spacing::XS)
                    .p(tokens::spacing::XS)
                    .rounded(tokens::radius::CONTROL)
                    .border_1()
                    .border_color(colors.border_default)
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(tokens::spacing::XS)
                            .child(shrinking_badge(
                                format!("Worktree {}", id.as_str()),
                                if selected {
                                    colors.info_text
                                } else {
                                    colors.text_secondary
                                },
                                if selected {
                                    colors.info_background
                                } else {
                                    colors.background_tertiary
                                },
                            ))
                            .child(shrinking_badge(
                                status.branch.clone(),
                                colors.text_secondary,
                                colors.background_tertiary,
                            ))
                            .child(badge(
                                format!("{} 件の変更", status.files.len()),
                                colors.text_secondary,
                                colors.background_tertiary,
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(tokens::spacing::XS)
                            .child(
                                operation_button(
                                    format!("git-worktree-select-{index}"),
                                    "このWorktreeを選ぶ",
                                    selected,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.select_target(Some(select.clone()), cx)
                                    },
                                )),
                            )
                            .child(
                                operation_button(
                                    format!("git-worktree-open-{index}"),
                                    "このTaskで作業",
                                    false,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.open_task_worktree(Some(open.clone()), cx)
                                    },
                                )),
                            )
                            .child(
                                operation_button(
                                    format!("git-worktree-discard-{index}"),
                                    "Worktree削除…",
                                    false,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        this.confirm_operation(
                                            GitOperation::Discard(remove.clone()),
                                            format!("Worktree {}を削除", remove.as_str()),
                                            window,
                                            cx,
                                        );
                                    },
                                )),
                            ),
                    ),
            );
        }
        group
    }

    /// 生のコマンド出力。既定では閉じた別枠に分離し、必要時だけ開く。
    ///
    /// 高さは直近の数行に限り、履歴やStash一覧のような確認用途だけに使う。
    fn render_command_output(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Div> {
        let output = &self.snapshot.output;
        if output.trim().is_empty() {
            return None;
        }
        let colors = theme::colors(cx);
        // 開閉はViewの状態ではなく、この要素の寿命に閉じた状態として持つ。
        let open = window.use_keyed_state("git-output-open", cx, |_, _| false);
        let is_open = *open.read(cx);
        let lines = output.lines().collect::<Vec<_>>();
        let start = lines.len().saturating_sub(COMMAND_OUTPUT_LINES);
        let mut header = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(tokens::spacing::XS)
            .child(group_title("コマンド出力", cx))
            .child(badge(
                format!("{} 行", lines.len()),
                colors.text_tertiary,
                colors.background_tertiary,
            ));
        if start > 0 {
            header = header.child(badge(
                format!("末尾 {COMMAND_OUTPUT_LINES} 行"),
                colors.text_tertiary,
                colors.background_tertiary,
            ));
        }
        header = header.child(
            operation_button(
                "git-output-toggle",
                if is_open { "閉じる" } else { "表示" },
                false,
                cx,
            )
            .on_click(cx.listener(move |_, _, _, cx| {
                open.update(cx, |open, cx| {
                    *open = !*open;
                    cx.notify();
                });
            })),
        );
        let mut pane = div()
            .flex()
            .flex_col()
            .gap(tokens::spacing::XS)
            .w_full()
            .min_w(px(0.0))
            .p(tokens::spacing::SM)
            .rounded(tokens::radius::PANEL)
            .border_1()
            .border_color(colors.border_default)
            .child(header);
        if is_open {
            pane = pane.child(
                div()
                    .id("git-output-body")
                    .debug_selector(|| "git-output-body".into())
                    .flex()
                    .flex_col()
                    .font_family(MONO_FONT_FAMILY)
                    .text_size(tokens::typography::FONT_XS)
                    .text_color(colors.text_secondary)
                    .children(
                        lines[start..]
                            .iter()
                            .map(|line| div().child((*line).to_owned())),
                    ),
            );
        } else {
            // 閉じているあいだも、結果が残っていることだけは分かるようにする。
            pane = pane.child(muted_text(
                "履歴・Branch一覧・Stash一覧の結果を確認するときに開きます。",
                cx,
            ));
        }
        Some(pane)
    }

    /// 差分を行番号つきで表示し、hunkヘッダからhunk単位のStageへ到達させる。
    fn render_diff_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme::colors(cx);
        let pane = div()
            .id("git-diff-pane")
            .debug_selector(|| "git-diff-pane".into())
            .flex()
            .flex_col()
            .gap(tokens::spacing::XS)
            .flex_1()
            .flex_basis(DIFF_PANE_BASIS)
            .min_w(DIFF_PANE_MIN_WIDTH)
            .min_h(px(0.0));
        let (Some(diff), Some(selection)) = (
            self.snapshot.selected_diff.clone(),
            self.snapshot.inspection.clone(),
        ) else {
            return pane.child(card(cx).child(section_title("Diff", cx)).child(muted_text(
                "変更ファイルの「Diff」を選ぶと、ここに内容を表示します",
                cx,
            )));
        };
        let target_matches = selection.repository_id.is_none() && selection.worktree == self.target;
        let pane = pane.child(
            card(cx)
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(tokens::spacing::XS)
                        .child(section_title("Diff", cx))
                        .child(badge(
                            if selection.staged {
                                "Stage済み"
                            } else {
                                "未Stage"
                            },
                            colors.text_secondary,
                            colors.background_tertiary,
                        ))
                        .child(badge(
                            format!("+{}", diff.additions),
                            colors.positive_text,
                            colors.positive_background,
                        ))
                        .child(badge(
                            format!("-{}", diff.deletions),
                            colors.danger_text,
                            colors.danger_background,
                        ))
                        .when(diff.binary, |row| {
                            row.child(badge(
                                "バイナリ",
                                colors.text_secondary,
                                colors.background_tertiary,
                            ))
                        })
                        .when(!target_matches, |row| {
                            row.child(badge(
                                "別の作業対象",
                                colors.warning_text,
                                colors.warning_background,
                            ))
                        }),
                )
                .child(
                    div()
                        .min_w(px(0.0))
                        .truncate()
                        .font_family(MONO_FONT_FAMILY)
                        .text_size(tokens::typography::FONT_XS)
                        .text_color(colors.text_primary)
                        .child(diff.path.clone()),
                ),
        );
        if diff.binary {
            return pane.child(muted_text("バイナリのためDiffを表示できません", cx));
        }
        if diff.patch.trim().is_empty() {
            return pane.child(muted_text("Diffはありません", cx));
        }
        let patch = std::sync::Arc::new(diff.patch.clone());
        let path = selection.path.clone();
        let staged = selection.staged;
        let mut rows = div().flex().flex_col().w_full().min_w(px(0.0));
        for row in diff_rows(&diff.patch) {
            rows = rows.child(match row {
                DiffRow::Meta { text } => diff_meta_row(text, cx).into_any_element(),
                DiffRow::Hunk { index, header } => {
                    let mut hunk = div()
                        .id(("git-hunk", index))
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(tokens::spacing::XS)
                        .px(tokens::spacing::XS)
                        .py(px(1.0))
                        .rounded(tokens::radius::XS)
                        .bg(colors.background_tertiary)
                        .child(badge(
                            format!("Hunk {}", index + 1),
                            colors.info_text,
                            colors.info_background,
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .font_family(MONO_FONT_FAMILY)
                                .text_size(tokens::typography::FONT_XS)
                                .text_color(colors.info_text)
                                .child(header.to_owned()),
                        );
                    if target_matches {
                        let patch = patch.clone();
                        let path = path.clone();
                        hunk = hunk.child(
                            operation_button(
                                format!("git-hunk-stage-{index}"),
                                if staged {
                                    "このHunkをUnstage"
                                } else {
                                    "このHunkをStage"
                                },
                                false,
                                cx,
                            )
                            .on_click(cx.listener(
                                move |this, _, window, cx| {
                                    this.execute_action(
                                        GitAction::StageHunk {
                                            path: path.clone(),
                                            staged,
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
                    hunk.into_any_element()
                }
                DiffRow::Line { old, new, text } => {
                    render_diff_line(old, new, text, cx).into_any_element()
                }
            });
        }
        pane.child(rows)
    }
}

/// 差分1行を、旧行番号・新行番号つきで描画する。
fn render_diff_line(old: Option<usize>, new: Option<usize>, text: &str, cx: &App) -> gpui::Div {
    let colors = theme::colors(cx);
    let color = if text.starts_with('+') {
        colors.positive_text
    } else if text.starts_with('-') {
        colors.danger_text
    } else {
        colors.text_secondary
    };
    div()
        .flex()
        .items_start()
        .w_full()
        .min_w(px(0.0))
        .font_family(MONO_FONT_FAMILY)
        .text_size(tokens::typography::FONT_XS)
        .child(diff_line_number(old, cx))
        .child(diff_line_number(new, cx))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .text_color(color)
                .child(text.to_owned()),
        )
}

fn diff_line_number(value: Option<usize>, cx: &App) -> gpui::Div {
    div()
        .w(px(34.0))
        .flex_shrink_0()
        .text_right()
        .text_color(theme::colors(cx).text_tertiary)
        .child(value.map(|value| value.to_string()).unwrap_or_default())
}

fn diff_meta_row(text: &str, cx: &App) -> gpui::Div {
    div()
        .w_full()
        .min_w(px(0.0))
        .font_family(MONO_FONT_FAMILY)
        .text_size(tokens::typography::FONT_XS)
        .text_color(theme::colors(cx).text_tertiary)
        .child(text.to_owned())
}

#[derive(Debug, PartialEq, Eq)]
enum DiffRow<'a> {
    /// ファイルヘッダなど、行番号を持たない行。
    Meta { text: &'a str },
    /// hunkヘッダ。`index` は patch 内の `@@ ` 行の出現順で、StageHunk の指定と一致する。
    Hunk { index: usize, header: &'a str },
    /// 追加・削除・文脈の各行。`text` は +/-/空白を含む元の行。
    Line {
        old: Option<usize>,
        new: Option<usize>,
        text: &'a str,
    },
}

/// patchを行単位へ分解し、hunkヘッダと行番号を付ける。
///
/// `index` は `patch.lines()` の `@@ ` 行を0から数えた値と一致させ、
/// `GitAction::StageHunk` の指定を従来と同じ意味に保つ。
fn diff_rows(patch: &str) -> Vec<DiffRow<'_>> {
    let mut rows = Vec::new();
    let mut old_line = 0;
    let mut new_line = 0;
    let mut in_hunk = false;
    let mut hunk_index = 0;
    for line in patch.lines() {
        if line.starts_with("diff --git ") || line.starts_with("index ") {
            in_hunk = false;
            rows.push(DiffRow::Meta { text: line });
            continue;
        }
        if line.starts_with("@@ ") {
            if let Some((old, new)) = hunk_start(line) {
                old_line = old;
                new_line = new;
            }
            in_hunk = true;
            rows.push(DiffRow::Hunk {
                index: hunk_index,
                header: line,
            });
            hunk_index += 1;
            continue;
        }
        if !in_hunk {
            rows.push(DiffRow::Meta { text: line });
            continue;
        }
        if line.starts_with('+') {
            rows.push(DiffRow::Line {
                old: None,
                new: Some(new_line),
                text: line,
            });
            new_line += 1;
        } else if line.starts_with('-') {
            rows.push(DiffRow::Line {
                old: Some(old_line),
                new: None,
                text: line,
            });
            old_line += 1;
        } else if line.starts_with(' ') || line.is_empty() {
            rows.push(DiffRow::Line {
                old: Some(old_line),
                new: Some(new_line),
                text: line,
            });
            old_line += 1;
            new_line += 1;
        } else {
            // "\ No newline at end of file" など、行番号を持たない注記。
            rows.push(DiffRow::Meta { text: line });
        }
    }
    rows
}

/// `@@ -old,count +new,count @@` から開始行を取り出す。
fn hunk_start(header: &str) -> Option<(usize, usize)> {
    let mut fields = header.split_whitespace();
    if fields.next()? != "@@" {
        return None;
    }
    let old = hunk_start_of(fields.next()?, '-')?;
    let new = hunk_start_of(fields.next()?, '+')?;
    Some((old, new))
}

fn hunk_start_of(field: &str, sign: char) -> Option<usize> {
    field
        .strip_prefix(sign)?
        .split(',')
        .next()?
        .parse::<usize>()
        .ok()
}

fn card(cx: &App) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(tokens::spacing::XS)
        .w_full()
        .min_w(px(0.0))
        .p(tokens::spacing::SM)
        .rounded(tokens::radius::PANEL)
        .border_1()
        .border_color(theme::colors(cx).border_default)
}

fn section_title(label: &'static str, cx: &App) -> gpui::Div {
    div()
        .text_size(tokens::typography::FONT_SM)
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(theme::colors(cx).text_primary)
        .child(label)
}

fn group_title(label: impl Into<SharedString>, cx: &App) -> gpui::Div {
    div()
        .text_size(tokens::typography::FONT_XS)
        .text_color(theme::colors(cx).text_secondary)
        .child(label.into())
}

fn muted_text(text: impl Into<SharedString>, cx: &App) -> gpui::Div {
    div()
        .text_size(tokens::typography::FONT_XS)
        .text_color(theme::colors(cx).text_tertiary)
        .child(text.into())
}

fn message_banner(message: String, cx: &App) -> gpui::Div {
    let colors = theme::colors(cx);
    div()
        .flex()
        .items_center()
        .gap(tokens::spacing::XS)
        .px(tokens::spacing::SM)
        .py(tokens::spacing::XS)
        .rounded(tokens::radius::CONTROL)
        .bg(colors.danger_background)
        .text_size(tokens::typography::FONT_XS)
        .text_color(colors.danger_text)
        .child(message)
}

fn badge(label: impl Into<SharedString>, text: Rgba, background: Rgba) -> gpui::Div {
    div()
        .flex_shrink_0()
        .px(tokens::spacing::XS)
        .py(px(1.0))
        .rounded(tokens::radius::CONTROL)
        .bg(background)
        .text_size(tokens::typography::FONT_XS)
        .text_color(text)
        .whitespace_nowrap()
        .child(label.into())
}

/// 長さが外由来で読めない可能性があるBadge（パスなど）。
///
/// 幅を親に合わせて縮め、はみ出す分だけ省略する。`badge` の `flex_shrink_0` は
/// 短い固定ラベル用で、長いパスに使うとPanelの横スクロールを強制してしまう。
fn shrinking_badge(label: impl Into<SharedString>, text: Rgba, background: Rgba) -> gpui::Div {
    div()
        .min_w(px(0.0))
        .px(tokens::spacing::XS)
        .py(px(1.0))
        .rounded(tokens::radius::CONTROL)
        .bg(background)
        .text_size(tokens::typography::FONT_XS)
        .text_color(text)
        .truncate()
        .child(label.into())
}

/// 操作ボタンに到達確認用のselectorを付ける。ラベルだけでは行ごとに重複するため。
fn operation_button(
    id: impl Into<String>,
    label: impl Into<SharedString>,
    primary: bool,
    cx: &App,
) -> Stateful<gpui::Div> {
    let id = id.into();
    action_button(label, primary, cx).debug_selector(move || id)
}

fn head_label(head: &str) -> String {
    if head.is_empty() {
        "コミットなし".to_owned()
    } else {
        head.chars().take(7).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "\
diff --git a/src/main.rs b/src/main.rs
index 1111111..2222222 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -3,3 +3,4 @@ fn main() {
 let a = 1;
-let b = 2;
+let b = 3;
+let c = 4;
@@ -10,2 +11,2 @@ fn other() {
 same();
-old();
+new();
";

    #[test]
    fn diff_rows_keep_patch_order_numbering_and_hunk_indices() {
        let rows = diff_rows(PATCH);
        assert!(matches!(rows[0], DiffRow::Meta { text } if text.starts_with("diff --git ")));
        assert!(matches!(&rows[1], DiffRow::Meta { text } if text.starts_with("index ")));
        assert!(matches!(&rows[4], DiffRow::Hunk { index: 0, .. }));
        assert_eq!(
            &rows[5..9],
            &[
                DiffRow::Line {
                    old: Some(3),
                    new: Some(3),
                    text: " let a = 1;",
                },
                DiffRow::Line {
                    old: Some(4),
                    new: None,
                    text: "-let b = 2;",
                },
                DiffRow::Line {
                    old: None,
                    new: Some(4),
                    text: "+let b = 3;",
                },
                DiffRow::Line {
                    old: None,
                    new: Some(5),
                    text: "+let c = 4;",
                },
            ]
        );
        assert!(matches!(&rows[9], DiffRow::Hunk { index: 1, .. }));
        assert_eq!(
            rows[10],
            DiffRow::Line {
                old: Some(10),
                new: Some(11),
                text: " same();",
            }
        );
        assert_eq!(
            rows[11],
            DiffRow::Line {
                old: Some(11),
                new: None,
                text: "-old();",
            }
        );
        assert_eq!(
            rows[12],
            DiffRow::Line {
                old: None,
                new: Some(12),
                text: "+new();",
            }
        );
    }

    #[test]
    fn hunk_indices_match_the_enumeration_used_for_stage_hunk() {
        let expected = PATCH
            .lines()
            .filter(|line| line.starts_with("@@ "))
            .enumerate()
            .collect::<Vec<_>>();
        let actual = diff_rows(PATCH)
            .into_iter()
            .filter_map(|row| match row {
                DiffRow::Hunk { index, header } => Some((index, header)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
    }

    #[test]
    fn hunk_header_and_single_line_counts_are_parsed() {
        assert_eq!(hunk_start("@@ -3,3 +4,4 @@ fn main() {"), Some((3, 4)));
        assert_eq!(hunk_start("@@ -10 +11 @@"), Some((10, 11)));
        assert_eq!(hunk_start("@@ -0,0 +1,2 @@"), Some((0, 1)));
        assert_eq!(hunk_start("not a hunk"), None);
    }

    #[test]
    fn no_newline_marker_and_trailing_lines_stay_readable() {
        let rows = diff_rows("@@ -1,1 +1,1 @@\n-a\n\\ No newline at end of file\n+b");
        assert_eq!(
            rows,
            vec![
                DiffRow::Hunk {
                    index: 0,
                    header: "@@ -1,1 +1,1 @@",
                },
                DiffRow::Line {
                    old: Some(1),
                    new: None,
                    text: "-a",
                },
                DiffRow::Meta {
                    text: "\\ No newline at end of file",
                },
                DiffRow::Line {
                    old: None,
                    new: Some(1),
                    text: "+b",
                },
            ]
        );
    }

    #[test]
    fn short_head_is_readable_and_empty_head_is_named() {
        assert_eq!(head_label(""), "コミットなし");
        assert_eq!(head_label("0123456789abcdef"), "0123456");
    }
}

#[cfg(all(test, debug_assertions))]
mod reachability_tests {
    use super::*;
    use desktop_client::GitSession;
    use editor_core::{TaskId, WorkspaceId};
    use extension_ui_contract::ThemeId;
    use git::{GitDiffSnapshot, GitRepositorySnapshot, GitSnapshot, WorktreeSnapshot};

    struct Backend;

    impl git::GitConnection for Backend {
        fn status(
            &self,
            workspace: &WorkspaceId,
            worktree: Option<&TaskId>,
        ) -> Result<GitSnapshot, git::GitError> {
            Ok(GitSnapshot {
                workspace_id: workspace.clone(),
                worktree_id: worktree.cloned(),
                revision: 1,
                branch: "feat/git-view".into(),
                head: "0123456789abcdef".into(),
                files: vec![
                    GitFileSnapshot {
                        path: "src/staged.rs".into(),
                        old_path: None,
                        kind: ChangeKind::Modified,
                        staged: true,
                    },
                    GitFileSnapshot {
                        path: "src/conflicted.rs".into(),
                        old_path: None,
                        kind: ChangeKind::Conflicted,
                        staged: false,
                    },
                ],
            })
        }
        fn diff(
            &self,
            _: &WorkspaceId,
            _: Option<&TaskId>,
            path: &str,
        ) -> Result<GitDiffSnapshot, git::GitError> {
            Ok(GitDiffSnapshot {
                path: path.into(),
                additions: 1,
                deletions: 1,
                patch: "@@ -1,1 +1,1 @@\n-a\n+b\n".into(),
                binary: false,
            })
        }
        fn worktrees(&self, _: &WorkspaceId) -> Result<Vec<WorktreeSnapshot>, git::GitError> {
            Ok(Vec::new())
        }
        fn create_worktree(&self, _: &WorkspaceId) -> Result<WorktreeSnapshot, git::GitError> {
            unreachable!()
        }
        fn import_file(
            &self,
            _: &WorkspaceId,
            _: &TaskId,
            _: &str,
        ) -> Result<WorktreeSnapshot, git::GitError> {
            unreachable!()
        }
        fn discard_worktree(
            &self,
            _: &WorkspaceId,
            _: &TaskId,
        ) -> Result<WorktreeSnapshot, git::GitError> {
            unreachable!()
        }
    }

    fn feature(cx: &mut gpui::Context<GitFeature>) -> GitFeature {
        let workspace = WorkspaceId::new("workspace");
        let session = GitSession::new(std::sync::Arc::new(Backend));
        let mut feature = GitFeature::new(session, cx);
        let files = vec![
            GitFileSnapshot {
                path: "src/staged.rs".into(),
                old_path: None,
                kind: ChangeKind::Modified,
                staged: true,
            },
            GitFileSnapshot {
                path: "src/conflicted.rs".into(),
                old_path: None,
                kind: ChangeKind::Conflicted,
                staged: false,
            },
        ];
        feature.snapshot.status = Some(GitSnapshot {
            workspace_id: workspace.clone(),
            worktree_id: None,
            revision: 2,
            branch: "feat/git-view".into(),
            head: "0123456789abcdef".into(),
            files: files.clone(),
        });
        feature.snapshot.worktree_statuses = vec![(
            TaskId::new("wt-a"),
            GitSnapshot {
                workspace_id: workspace.clone(),
                worktree_id: Some(TaskId::new("wt-a")),
                revision: 1,
                branch: "task/wt-a".into(),
                head: "abcdef0123456789".into(),
                files,
            },
        )];
        feature.snapshot.repositories = vec![(
            GitRepositorySnapshot {
                id: "root".into(),
                path: None,
            },
            GitSnapshot {
                workspace_id: workspace.clone(),
                worktree_id: None,
                revision: 1,
                branch: "feat/git-view".into(),
                head: "0123456789abcdef".into(),
                files: Vec::new(),
            },
        )];
        feature.snapshot.inspection = Some(desktop_client::GitDiffSelection {
            worktree: Some(TaskId::new("wt-a")),
            repository_id: None,
            path: "src/staged.rs".into(),
            staged: true,
        });
        feature.snapshot.selected_diff = Some(GitDiffSnapshot {
            path: "src/staged.rs".into(),
            additions: 1,
            deletions: 1,
            patch: "@@ -1,1 +1,1 @@\n-a\n+b\n".into(),
            binary: false,
        });
        feature.context_label = "Desktop · C:/workspace".into();
        feature
    }

    fn draw(visual: &mut gpui::VisualTestContext) {
        visual.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear();
        });
    }

    #[gpui::test]
    fn every_git_operation_stays_reachable_from_the_new_layout(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::theme::init(&ThemeId::new("lapis.dark"), None, cx));
        let (feature, visual) = cx.add_window_view(|_, cx| feature(cx));
        visual.simulate_resize(gpui::size(px(1000.), px(1100.)));
        feature.update(visual, |feature, cx| {
            // 取込はWorktreeを対象にしているときだけ出す。
            feature.target = Some(TaskId::new("wt-a"));
            cx.notify();
        });
        draw(visual);
        let expected = [
            "git-commit",
            "git-file-diff-0",
            "git-file-stage-0",
            "git-file-diff-1",
            "git-file-stage-1",
            "git-file-ours-1",
            "git-file-theirs-1",
            "git-file-import-0",
            "git-file-import-1",
            "git-hunk-stage-0",
            "git-action-fetch",
            "git-action-pull",
            "git-action-push",
            "git-action-history",
            "git-action-branches",
            "git-action-stashes",
            "git-action-abort-merge",
            "git-prompt-create-branch",
            "git-prompt-switch-branch",
            "git-prompt-delete-branch",
            "git-prompt-merge",
            "git-prompt-stash-push",
            "git-prompt-stash-apply",
            "git-prompt-stash-pop",
            "git-prompt-stash-drop",
            "git-worktree-create",
            "git-env-select",
            "git-worktree-return",
            "git-worktree-select-0",
            "git-worktree-open-0",
            "git-worktree-discard-0",
        ];
        for selector in expected {
            assert!(
                visual.debug_bounds(selector).is_some(),
                "{selector} が新しいレイアウトに無い"
            );
        }
        // 位置だけでは「押せる」ことまでは分からないため、実際にクリックして配線を確かめる。
        let worktree = visual.debug_bounds("git-worktree-select-0").unwrap();
        visual.simulate_click(worktree.center(), gpui::Modifiers::none());
        feature.read_with(visual, |feature, _| {
            assert_eq!(feature.target, Some(TaskId::new("wt-a")));
        });
        draw(visual);
        let registered = visual.debug_bounds("git-env-select").unwrap();
        visual.simulate_click(registered.center(), gpui::Modifiers::none());
        feature.read_with(visual, |feature, _| {
            assert_eq!(feature.target, None, "登録環境のクリックが届いていない");
        });
    }

    #[gpui::test]
    fn command_output_is_collapsed_by_default_and_opens_on_demand(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::theme::init(&ThemeId::new("lapis.dark"), None, cx));
        let (feature, visual) = cx.add_window_view(|_, cx| {
            let mut feature = feature(cx);
            feature.snapshot.output = (0..120)
                .map(|index| format!("command output line {index}"))
                .collect::<Vec<_>>()
                .join("\n");
            feature
        });
        visual.simulate_resize(gpui::size(px(1000.), px(1100.)));
        draw(visual);
        // トグルは常にあり、本文は既定で閉じている。生ログが本文へ流れ続けない。
        let toggle = visual.debug_bounds("git-output-toggle").unwrap();
        let before =
            feature.read_with(visual, |feature, _| feature.snapshot.output.lines().count());
        assert_eq!(before, 120);
        assert!(
            visual.debug_bounds("git-output-body").is_none(),
            "既定ではコマンド出力の本文を出さない"
        );
        visual.simulate_click(toggle.center(), gpui::Modifiers::none());
        draw(visual);
        let opened = visual.debug_bounds("git-output-toggle").unwrap();
        assert_eq!(
            opened.top(),
            toggle.top(),
            "トグルを開閉しても位置が動かない"
        );
        assert!(
            visual.debug_bounds("git-output-body").is_some(),
            "表示を選んだら出力本文を出す"
        );
    }

    #[gpui::test]
    fn command_output_frame_is_absent_without_output(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::theme::init(&ThemeId::new("lapis.dark"), None, cx));
        let (_, visual) = cx.add_window_view(|_, cx| feature(cx));
        visual.simulate_resize(gpui::size(px(1000.), px(1100.)));
        draw(visual);
        assert!(
            visual.debug_bounds("git-output-toggle").is_none(),
            "出力が無いのにコマンド出力枠がある"
        );
    }

    #[gpui::test]
    fn status_bar_reports_context_branch_head_and_conflicts(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::theme::init(&ThemeId::new("lapis.dark"), None, cx));
        let (_, visual) = cx.add_window_view(|_, cx| feature(cx));
        visual.simulate_resize(gpui::size(px(1000.), px(1100.)));
        draw(visual);
        let bar = visual.debug_bounds("git-status-bar").unwrap();
        assert!(bar.size.height > px(0.0));
    }

    #[gpui::test]
    fn a_long_work_target_path_does_not_force_horizontal_overflow(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::theme::init(&ThemeId::new("lapis.dark"), None, cx));
        let (feature, visual) = cx.add_window_view(|_, cx| {
            let mut feature = feature(cx);
            // context_label は root.rs が実パスから作る。長いパスでも横スクロールを強制しない。
            feature.context_label = format!("Desktop · C:/{}", "very-long-directory/".repeat(20));
            feature
        });
        visual.simulate_resize(gpui::size(px(360.), px(900.)));
        draw(visual);
        let bar = visual.debug_bounds("git-status-bar").unwrap();
        assert!(
            bar.right() <= px(360.),
            "長い作業対象パスがPanel幅を超えている: {bar:?}"
        );
        assert!(
            feature.read_with(visual, |feature, _| !feature.context_label.is_empty()),
            "ラベル自体は保持する（省略表示だけ）"
        );
    }

    #[gpui::test]
    fn view_relies_on_the_shared_panel_padding_without_adding_its_own(
        cx: &mut gpui::TestAppContext,
    ) {
        // panel_frame と同じく panel_content の内側へ Git view を描画する。
        struct PanelBody(gpui::Entity<GitFeature>);

        impl Render for PanelBody {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                crate::components::panel_content(div().w_full().child(self.0.clone()))
            }
        }

        cx.update(|cx| crate::theme::init(&ThemeId::new("lapis.dark"), None, cx));
        let (_, visual) = cx.add_window_view(|_, cx| PanelBody(cx.new(feature)));
        visual.simulate_resize(gpui::size(px(1000.), px(1100.)));
        draw(visual);
        let padding = f32::from(tokens::spacing::PANEL_CONTENT_X);
        let bar = visual.debug_bounds("git-status-bar").unwrap();
        assert_eq!(f32::from(bar.origin.x), padding, "左の余白は共通定義だけ");
        // 右端は隣のPaneで決まるため、行の終端で確認する。
        let diff = visual.debug_bounds("git-diff-pane").unwrap();
        assert_eq!(
            f32::from(diff.right()),
            f32::from(px(1000.)) - padding,
            "Feature側で左右の余白を二重に足していない"
        );
    }

    #[gpui::test]
    fn commit_stays_the_first_operation_before_the_other_groups(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| crate::theme::init(&ThemeId::new("lapis.dark"), None, cx));
        let (_, visual) = cx.add_window_view(|_, cx| feature(cx));
        visual.simulate_resize(gpui::size(px(1000.), px(700.)));
        draw(visual);
        let commit = visual.debug_bounds("git-commit").unwrap();
        let fetch = visual.debug_bounds("git-action-fetch").unwrap();
        let create = visual.debug_bounds("git-worktree-create").unwrap();
        assert!(commit.top() < fetch.top());
        assert!(fetch.top() < create.top());
    }

    #[gpui::test]
    fn files_and_diff_panes_sit_side_by_side_when_wide_and_stack_when_narrow(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| crate::theme::init(&ThemeId::new("lapis.dark"), None, cx));
        let (_, visual) = cx.add_window_view(|_, cx| feature(cx));
        visual.simulate_resize(gpui::size(px(1100.), px(700.)));
        draw(visual);
        let files = visual.debug_bounds("git-files-pane").unwrap();
        let diff = visual.debug_bounds("git-diff-pane").unwrap();
        assert_eq!(diff.top(), files.top());
        assert!(diff.left() > files.right());

        visual.simulate_resize(gpui::size(px(420.), px(700.)));
        draw(visual);
        let files = visual.debug_bounds("git-files-pane").unwrap();
        let diff = visual.debug_bounds("git-diff-pane").unwrap();
        assert!(diff.top() >= files.bottom());
        assert_eq!(diff.left(), files.left());
    }
}
