use super::*;

impl Editor {
    pub(super) fn render_git_content(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut content = div()
            .id("git-scroll")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .scrollable(ScrollAxis::Vertical)
            .p_2()
            .gap_2();
        if let Some(status) = self.git.session.status() {
            content = content.child(
                div()
                    .text_size(px(10.0))
                    .text_color(theme::colors().subtle)
                    .child(format!(
                        "SHARED · {} · {} changes",
                        status.branch,
                        status.files.len()
                    )),
            );
            for (index, file) in status.files.iter().take(40).enumerate() {
                let path = file.path.clone();
                content = content.child(
                    div()
                        .id(("git-file", index))
                        .min_h(px(27.0))
                        .px_2()
                        .rounded(px(5.0))
                        .flex()
                        .items_center()
                        .gap_2()
                        .hover(|style| style.bg(theme::colors().surface_hover))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_git_diff(path.clone(), cx);
                        }))
                        .child(
                            div()
                                .text_size(px(10.0))
                                .text_color(change_color(file.kind))
                                .child(change_label(file.kind)),
                        )
                        .child(
                            div()
                                .text_size(px(10.0))
                                .text_color(theme::colors().muted)
                                .child(file.path.to_string_lossy().into_owned()),
                        ),
                );
            }
        } else {
            content = content.child(panel_empty_state(
                "⑂",
                "Git repositoryを確認中です",
                "機械可読statusだけを表示します",
            ));
        }
        for (worktree_index, (task_id, status)) in
            self.git.session.worktree_statuses().iter().enumerate()
        {
            let discard_task = task_id.clone();
            content = content.child(
                div()
                    .mt_2()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(theme::colors().assistant_accent)
                            .child(format!("WORKTREE · {} changes", status.files.len())),
                    )
                    .child(div().flex_1())
                    .child(task_action_button("破棄", false).on_click(cx.listener(
                        move |this, _, _, cx| this.discard_worktree(discard_task.clone(), cx),
                    ))),
            );
            for (file_index, file) in status.files.iter().take(30).enumerate() {
                let task_for_diff = task_id.clone();
                let task_for_import = task_id.clone();
                let diff_path = file.path.clone();
                let import_path = file.path.clone();
                content = content.child(
                    div()
                        .id(("worktree-file", worktree_index * 100 + file_index))
                        .min_h(px(29.0))
                        .px_2()
                        .rounded(px(5.0))
                        .flex()
                        .items_center()
                        .gap_2()
                        .hover(|style| style.bg(theme::colors().surface_hover))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_worktree_diff(task_for_diff.clone(), diff_path.clone(), cx);
                        }))
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(10.0))
                                .text_color(theme::colors().muted)
                                .child(file.path.to_string_lossy().into_owned()),
                        )
                        .child(task_action_button("取込", true).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.import_worktree_file(
                                    task_for_import.clone(),
                                    import_path.clone(),
                                    cx,
                                );
                            },
                        ))),
                );
            }
        }
        if let Some(diff) = self.git.session.selected_diff() {
            content = content
                .child(
                    div()
                        .mt_2()
                        .text_size(px(10.0))
                        .text_color(theme::colors().subtle)
                        .child(format!(
                            "DIFF · {} · +{} -{}",
                            diff.path.display(),
                            diff.additions,
                            diff.deletions
                        )),
                )
                .children(diff.patch.lines().take(80).map(|line| {
                    div()
                        .text_size(px(9.0))
                        .text_color(if line.starts_with('+') {
                            theme::colors().diff_added
                        } else if line.starts_with('-') {
                            theme::colors().diff_removed
                        } else {
                            theme::colors().subtle
                        })
                        .child(line.to_owned())
                }));
        }
        div().flex().flex_1().min_h(px(0.0)).child(content)
    }
}
