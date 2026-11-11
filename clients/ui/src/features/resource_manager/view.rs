use crate::{components::action_button, theme};
use gpui::{Context, Render, Window, div, prelude::*, px};
use process::{ProcessOrigin, ProcessState};

use super::{ResourceManagerFeature, ResourceRow, format_memory, kind_label, rows};

impl Render for ResourceManagerFeature {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.note_rendered(cx);
        let sampled = self.state.snapshot.roots.len();
        let mut content = div()
            .id("resource-manager")
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .text_size(px(12.0))
            .text_color(theme::colors(cx).text_primary)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child("Processes"),
                            )
                            .child(
                                div()
                                    .text_color(theme::colors(cx).text_secondary)
                                    .child(format!("{sampled} owned roots")),
                            ),
                    )
                    .child(
                        action_button("Refresh", false, cx)
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme::colors(cx).text_secondary)
                    .child("Only processes spawned by Lapis and their descendants are shown."),
            );
        let (refresh_status, terminate_status) = self.state.status_lines();
        if let Some(message) = refresh_status {
            content = content.child(
                div()
                    .text_color(theme::colors(cx).danger_text)
                    .child(message.to_owned()),
            );
        }
        if let Some(error) = terminate_status {
            content = content.child(
                div()
                    .text_color(theme::colors(cx).danger_text)
                    .child(format!("Termination failed: {error}")),
            );
        }
        if self.state.snapshot.roots.is_empty() {
            return content.child(
                div()
                    .text_color(theme::colors(cx).text_secondary)
                    .child("No managed processes. Terminal, task, and language server usage appears here while they run."),
            );
        }
        content = content.children(
            rows(&self.state.snapshot)
                .into_iter()
                .enumerate()
                .map(|(index, row)| match row {
            ResourceRow::Origin(origin) => div()
                .id(("resource-origin", index))
                .mt_2()
                .text_size(px(11.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme::colors(cx).text_secondary)
                .child(match origin {
                    ProcessOrigin::Host => "Host",
                    ProcessOrigin::Desktop => "Desktop",
                }),
            ResourceRow::Root(root) => {
                let pending = self.state.pending_terminate.as_ref() == Some(&root.root_id);
                let mut row = div()
                    .id(("process-root", index))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .rounded(px(5.0))
                    .bg(theme::colors(cx).background_tertiary)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(root.label.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(theme::colors(cx).text_secondary)
                                    .child(kind_label(root.kind)),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(theme::colors(cx).text_secondary)
                                    .child(match root.state {
                                        ProcessState::Running => "running",
                                        ProcessState::Exited => "exited",
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(theme::colors(cx).text_secondary)
                                    .child(format!("PID {}", root.pid)),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(theme::colors(cx).text_secondary)
                                    .child(match &root.owner.id {
                                        Some(id) => format!("{}: {id}", kind_label(root.owner.kind)),
                                        None => kind_label(root.owner.kind).to_owned(),
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .text_size(px(11.0))
                            .text_color(theme::colors(cx).text_secondary)
                            .child(format!("CPU {:.1}%", root.cpu_percent()))
                            .child(format!("MEM {}", format_memory(root.memory_bytes)))
                            .when(root.terminable, |row| {
                                row.child(if pending {
                                    div().flex().gap_1().child(
                                        action_button("Terminate tree", true, cx).on_click(
                                            cx.listener(|this, _, _, cx| {
                                                this.confirm_terminate(cx)
                                            }),
                                        ),
                                    ).child(action_button("Cancel", false, cx).on_click(
                                        cx.listener(|this, _, _, cx| this.cancel_terminate(cx)),
                                    ))
                                } else {
                                    div().child(action_button("Terminate", false, cx).on_click(
                                        cx.listener({
                                            let root_id = root.root_id.clone();
                                            move |this, _, _, cx| {
                                                this.request_terminate(root_id.clone(), cx)
                                            }
                                        }),
                                    ))
                                })
                            }),
                    );
                if pending {
                    row = row.child(
                        div()
                            .text_size(px(11.0))
                            .text_color(theme::colors(cx).danger_text)
                            .child("This terminates the owned process tree, including child processes."),
                    );
                }
                row
            }
            ResourceRow::Child { root, node } => div()
                .id(("resource-child", index))
                .flex()
                .items_center()
                .gap_3()
                .pl(px(12.0 * f32::from(node.depth)))
                .text_size(px(11.0))
                .text_color(theme::colors(cx).text_secondary)
                .child(format!("{} (PID {})", node.name, node.pid))
                .child(format!("CPU {:.1}%", node.cpu_percent()))
                .child(format!("MEM {}", format_memory(node.memory_bytes)))
                .when(root.state == ProcessState::Exited, |row| row.child("exited")),
        }));
        content
    }
}
