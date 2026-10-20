use super::state::{TasksFeature, task_status_color, truncate_chars};
use crate::{
    components::{ScrollAxis, action_button, panel_empty_state, scroll_viewport},
    theme,
};
use gpui::{Context, Render, Window, div, prelude::*, px};
use lapis_task::{ExecutionStatus, TaskControl, TaskMode};

impl TasksFeature {
    pub(super) fn render_assistant_content(&self, cx: &mut Context<Self>) -> gpui::Div {
        let records = &self.records;
        let mut task_list = div().id("task-list").flex().flex_col().gap_1();
        for (task_index, record) in records.iter().take(12).enumerate() {
            let execution_id = record.execution.id.clone();
            let selected = self.selected_execution.as_ref() == Some(&execution_id);
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

        let task_list = scroll_viewport(
            "task-list-scroll",
            ScrollAxis::Vertical,
            &self.scroll_states.task_list,
            task_list,
        )
        .max_h(px(170.0));

        let selected = self.selected_execution.as_ref().and_then(|execution_id| {
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
                    .child(
                        action_button("承認", true).on_click(cx.listener(|this, _, _, cx| {
                            this.control_selected_task(TaskControl::Approve, cx);
                        })),
                    )
                    .child(
                        action_button("拒否", false).on_click(cx.listener(|this, _, _, cx| {
                            this.control_selected_task(TaskControl::Decline, cx);
                        })),
                    );
            }
            if status == ExecutionStatus::WaitingForInput {
                actions = actions.child(
                    action_button("Clipboard で回答", true)
                        .on_click(cx.listener(|this, _, _, cx| this.reply_to_selected_task(cx))),
                );
            }
            if !status.is_terminal() {
                actions = actions.child(action_button("取消", false).on_click(cx.listener(
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
                .child(
                    scroll_viewport(
                        "task-events-scroll",
                        ScrollAxis::Vertical,
                        &self.scroll_states.task_events,
                        events,
                    )
                    .flex_1()
                    .min_h(px(0.0)),
                );
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
                    .child(action_button("＋ Default", true).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.start_codex_task(TaskMode::Default, false, cx);
                        },
                    )))
                    .child(
                        action_button("Plan", false).on_click(cx.listener(|this, _, _, cx| {
                            this.start_codex_task(TaskMode::Plan, false, cx);
                        })),
                    )
                    .child(
                        action_button("WT", false).on_click(cx.listener(|this, _, _, cx| {
                            this.start_codex_task(TaskMode::Default, true, cx);
                        })),
                    ),
            )
            .child(task_list)
            .child(detail)
    }
}
impl Render for TasksFeature {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                action_button("Start Codex", true).on_click(cx.listener(|this, _, _, cx| {
                    let prompt = this
                        .selected_text
                        .clone()
                        .or_else(|| cx.read_from_clipboard().and_then(|item| item.text()))
                        .unwrap_or_else(|| "このWorkspaceを読み取り専用で調査してください".into());
                    this.start(prompt, TaskMode::Default, false, cx);
                })),
            )
            .child(self.message.clone())
            .child(self.render_assistant_content(cx))
    }
}
