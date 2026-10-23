use super::TerminalFeature;
use crate::{
    components::{ScrollAxis, action_button, scroll_viewport},
    theme,
};
use gpui::{Context, Render, Window, canvas, div, prelude::*, px};
use lapis_terminal::TerminalShell;

impl Render for TerminalFeature {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.update_screens();
        let active = self.active_id();
        let rows = self
            .active_screen()
            .map(|screen| screen.rows())
            .unwrap_or_default();
        let cursor = self.active_screen().and_then(|screen| screen.cursor());
        let output = self
            .terminals
            .iter()
            .find(|terminal| Some(&terminal.id) == active.as_ref())
            .map(|terminal| (terminal.id.clone(), terminal.output_sequence));
        if output != self.visible_output {
            self.visible_output = output;
            if self.focus.is_focused(window)
                && let Some((row, _)) = cursor
            {
                let handle = self.scroll.handle();
                let height = handle.bounds().size.height;
                if height > px(0.0) {
                    let mut offset = handle.offset();
                    let top = px(f32::from(row) * 16.0);
                    let bottom = top + px(16.0);
                    if top + offset.y < px(0.0) {
                        offset.y = -top;
                    } else if bottom + offset.y > height {
                        offset.y = height - bottom;
                    }
                    handle.set_offset(offset);
                }
            }
        }
        let entity = cx.entity();
        let focus = self.focus.clone();
        let tabs = self
            .terminals
            .iter()
            .map(|terminal| (terminal.id.clone(), terminal.status))
            .collect::<Vec<_>>();
        let discovered = self.discovered.clone();
        let truncated = self
            .terminals
            .iter()
            .find(|terminal| Some(&terminal.id) == active.as_ref())
            .is_some_and(|terminal| terminal.output_truncated);
        div()
            .flex()
            .flex_col()
            .size_full()
            .track_focus(&self.focus)
            .key_context("Terminal")
            .on_key_down(cx.listener(Self::key_down))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .child(
                        action_button("80×24", false)
                            .on_click(cx.listener(|this, _, _, cx| this.resize(80, 24, cx))),
                    )
                    .child(
                        action_button("120×30", false)
                            .on_click(cx.listener(|this, _, _, cx| this.resize(120, 30, cx))),
                    )
                    .child(action_button("終了…", false).on_click(
                        cx.listener(|this, _, window, cx| this.confirm_terminate(window, cx)),
                    ))
                    .child(action_button("Shell", true).on_click(cx.listener(
                        |this, _, window, cx| {
                            this.start(cx);
                            window.focus(&this.focus);
                        },
                    )))
                    .child(action_button("PowerShell", false).on_click(cx.listener(
                        |this, _, window, cx| {
                            this.start_shell(TerminalShell::PowerShell, cx);
                            window.focus(&this.focus);
                        },
                    )))
                    .child(action_button("cmd", false).on_click(cx.listener(
                        |this, _, window, cx| {
                            this.start_shell(TerminalShell::Cmd, cx);
                            window.focus(&this.focus);
                        },
                    )))
                    .child(
                        action_button("Host Terminals", false)
                            .on_click(cx.listener(|this, _, _, cx| this.discover(cx))),
                    )
                    .child(action_button("Paste", false).on_click(cx.listener(
                        |this, _, window, cx| {
                            this.send_clipboard(cx);
                            window.focus(&this.focus);
                        },
                    )))
                    .child(action_button("Copy screen", false).on_click(cx.listener(
                        |this, _, _, cx| {
                            if let Some(screen) = this.active_screen() {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                    screen.rows().join("\n"),
                                ));
                            }
                        },
                    ))),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .children(tabs.into_iter().enumerate().map(|(index, (id, status))| {
                        let selected = active.as_ref() == Some(&id);
                        action_button(format!("{} - {status:?}", index + 1), selected).on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.selected = Some(id.clone());
                                window.focus(&this.focus);
                                cx.notify();
                            }),
                        )
                    })),
            )
            .children(discovered.into_iter().map(|terminal| {
                let label = format!("Attach: {} - {:?}", terminal.id.as_str(), terminal.status);
                action_button(label, false).on_click(cx.listener(move |this, _, window, cx| {
                    this.attach(terminal.id.clone(), cx);
                    window.focus(&this.focus);
                }))
            }))
            .child(self.message.clone())
            .when(truncated, |view| {
                view.child("過去の出力は一部省略されています。画面はHostの状態から復元します。")
            })
            .child(
                scroll_viewport(
                    "terminal-output",
                    ScrollAxis::Both,
                    &self.scroll,
                    div()
                        .relative()
                        .min_h(px(480.0))
                        .min_w(px(960.0))
                        .font_family("Cascadia Mono")
                        .text_size(px(13.0))
                        .line_height(px(16.0))
                        .text_color(theme::colors().text_primary)
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(|this, _, window, _| window.focus(&this.focus)),
                        )
                        .children(rows.into_iter().map(|row| {
                            div()
                                .h(px(16.0))
                                .whitespace_nowrap()
                                .child(if row.is_empty() { " ".to_owned() } else { row })
                        }))
                        .child(
                            canvas(
                                move |_, _, _| (),
                                move |bounds, _, window, cx| {
                                    window.handle_input(
                                        &focus,
                                        gpui::ElementInputHandler::new(bounds, entity.clone()),
                                        cx,
                                    );
                                    if focus.is_focused(window)
                                        && let Some((row, column)) = cursor
                                    {
                                        window.paint_quad(gpui::fill(
                                            gpui::Bounds::new(
                                                bounds.origin
                                                    + gpui::point(
                                                        px(f32::from(column) * 7.8),
                                                        px(f32::from(row) * 16.0),
                                                    ),
                                                gpui::size(px(7.8), px(16.0)),
                                            ),
                                            gpui::rgba(0xffffff55),
                                        ));
                                    }
                                },
                            )
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full(),
                        ),
                )
                .flex_1(),
            )
            .when(!self.marked.is_empty(), |view| {
                view.child(self.marked.clone())
            })
    }
}
