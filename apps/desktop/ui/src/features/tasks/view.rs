use super::*;

impl Editor {
    pub(super) fn render_assistant_content(&self, cx: &mut Context<Self>) -> gpui::Div {
        let records = self.tasks.session.records();
        let mut task_list = div()
            .id("task-list")
            .flex()
            .flex_col()
            .gap_1()
            .max_h(px(170.0))
            .scrollable(ScrollAxis::Vertical);
        for (task_index, record) in records.iter().take(12).enumerate() {
            let execution_id = record.execution.id.clone();
            let selected = self.tasks.selected_execution.as_ref() == Some(&execution_id);
            let status = record.execution.status;
            task_list = task_list.child(
                div()
                    .id(("task", task_index))
                    .min_h(px(42.0))
                    .p_2()
                    .rounded(px(6.0))
                    .bg(if selected {
                        theme::colors().button_background_selected
                    } else {
                        theme::colors().background_tertiary
                    })
                    .hover(|style| style.bg(theme::colors().button_background_hover))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_execution(execution_id.clone(), cx);
                    }))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(theme::colors().text_primary)
                            .child(truncate_chars(&record.task.title, 42)),
                    )
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(task_status_color(status))
                            .child(format!(
                                "{} · {} · {}",
                                status.label(),
                                record.execution.mode.label(),
                                record.execution.runner
                            )),
                    ),
            );
        }

        let selected = self
            .tasks
            .selected_execution
            .as_ref()
            .and_then(|execution_id| {
                records
                    .iter()
                    .find(|record| &record.execution.id == execution_id)
            });
        let mut detail = div().flex().flex_col().flex_1().min_h(px(0.0)).gap_2();
        if let Some(record) = selected {
            let status = record.execution.status;
            let mut actions = div().flex().items_center().gap_1();
            if status == ExecutionStatus::WaitingForApproval {
                actions = actions
                    .child(task_action_button("承認", true).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.control_selected_task(TaskControl::Approve, cx);
                        },
                    )))
                    .child(task_action_button("拒否", false).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.control_selected_task(TaskControl::Decline, cx);
                        },
                    )));
            }
            if status == ExecutionStatus::WaitingForInput {
                actions = actions.child(
                    task_action_button("Clipboard で回答", true)
                        .on_click(cx.listener(|this, _, _, cx| this.reply_to_selected_task(cx))),
                );
            }
            if !status.is_terminal() {
                actions = actions.child(task_action_button("取消", false).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.control_selected_task(TaskControl::Cancel, cx);
                    },
                )));
            }

            let mut events = div()
                .id("task-events")
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .scrollable(ScrollAxis::Vertical)
                .gap_1();
            let start = record.events.len().saturating_sub(80);
            for event in &record.events[start..] {
                let text = truncate_chars(event.event.display_text().trim(), 500);
                if text.is_empty() {
                    continue;
                }
                events = events.child(
                    div()
                        .p_2()
                        .rounded(px(5.0))
                        .bg(theme::colors().background_tertiary)
                        .text_size(px(10.0))
                        .text_color(theme::colors().text_secondary)
                        .child(text),
                );
            }
            detail = detail
                .child(
                    div()
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(task_status_color(status))
                                .child(status.label()),
                        )
                        .child(div().flex_1())
                        .child(actions),
                )
                .child(events);
        } else {
            detail = detail.child(panel_empty_state(
                "✦",
                "Task はまだありません",
                "選択範囲またはクリップボードを指示として開始できます",
            ));
        }

        div()
            .flex_1()
            .min_h(px(0.0))
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(theme::colors().text_tertiary)
                            .child("CODEX TASKS"),
                    )
                    .child(div().flex_1())
                    .child(task_action_button("＋ Default", true).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.start_codex_task(TaskMode::Default, false, cx);
                        },
                    )))
                    .child(task_action_button("Plan", false).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.start_codex_task(TaskMode::Plan, false, cx);
                        },
                    )))
                    .child(task_action_button("WT", false).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.start_codex_task(TaskMode::Default, true, cx);
                        },
                    ))),
            )
            .child(task_list)
            .child(detail)
    }
}
