use super::TerminalFeature;
use crate::{
    components::{ScrollAxis, action_button, scroll_viewport},
    theme,
};
use gpui::{Context, Render, Window, canvas, div, prelude::*, px};
use lapis_terminal::{TerminalCell, TerminalColor, TerminalRow, TerminalShell};

impl Render for TerminalFeature {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_id();
        let screen = self.active_screen().cloned();
        let rows = screen
            .as_ref()
            .map(|screen| screen.rows.clone())
            .unwrap_or_default();
        let cursor = screen.as_ref().map(|screen| screen.cursor);
        let output = self
            .terminals
            .iter()
            .find(|terminal| Some(&terminal.id) == active.as_ref())
            .map(|terminal| (terminal.id.clone(), terminal.revision));
        if output != self.visible_output {
            self.visible_output = output;
            if self.focus.is_focused(window)
                && let Some(row) = cursor.map(|cursor| cursor.row)
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
                    .child(action_button("Terminate", false).on_click(
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
                        move |_, _, _, cx| {
                            if let Some(screen) = &screen {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                    screen.plain_text(),
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
                        .children(rows.into_iter().map(render_row))
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
                                        && cursor.is_some_and(|cursor| cursor.visible)
                                    {
                                        let cursor = cursor.expect("cursor is visible");
                                        window.paint_quad(gpui::fill(
                                            gpui::Bounds::new(
                                                bounds.origin
                                                    + gpui::point(
                                                        px(f32::from(cursor.column) * 7.8),
                                                        px(f32::from(cursor.row) * 16.0),
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

fn render_row(row: TerminalRow) -> impl IntoElement {
    div()
        .flex()
        .h(px(16.0))
        .children(row.cells.into_iter().map(render_cell))
}

fn render_cell(cell: TerminalCell) -> impl IntoElement {
    let default_foreground = theme::colors().text_primary;
    let default_background = theme::colors().background_primary;
    let mut foreground = color(cell.foreground, default_foreground);
    let mut background = color(cell.background, default_background);
    if cell.inverse {
        std::mem::swap(&mut foreground, &mut background);
    }
    let mut view = div()
        .flex_none()
        .w(px(7.8 * f32::from(cell.width)))
        .h(px(16.0))
        .whitespace_nowrap()
        .text_color(foreground)
        .bg(background)
        .child(if cell.text.is_empty() {
            " ".to_owned()
        } else {
            cell.text
        });
    if cell.bold {
        view = view.font_weight(gpui::FontWeight::BOLD);
    }
    if cell.italic {
        view = view.italic();
    }
    if cell.underline {
        view = view.underline();
    }
    view
}

fn color(color: TerminalColor, default: gpui::Rgba) -> gpui::Rgba {
    match color {
        TerminalColor::Default => default,
        TerminalColor::Rgb { red, green, blue } => rgba(red, green, blue),
        TerminalColor::Indexed(index) => {
            let (red, green, blue) = indexed_color(index);
            rgba(red, green, blue)
        }
    }
}

fn rgba(red: u8, green: u8, blue: u8) -> gpui::Rgba {
    gpui::rgba((u32::from(red) << 24) | (u32::from(green) << 16) | (u32::from(blue) << 8) | 0xff)
}

fn indexed_color(index: u8) -> (u8, u8, u8) {
    const BASIC: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 49, 49),
        (13, 188, 121),
        (229, 229, 16),
        (36, 114, 200),
        (188, 63, 188),
        (17, 168, 205),
        (229, 229, 229),
        (102, 102, 102),
        (241, 76, 76),
        (35, 209, 139),
        (245, 245, 67),
        (59, 142, 234),
        (214, 112, 214),
        (41, 184, 219),
        (255, 255, 255),
    ];
    if let Some(color) = BASIC.get(index as usize) {
        return *color;
    }
    if (16..=231).contains(&index) {
        let value = index - 16;
        let component = |value: u8| if value == 0 { 0 } else { 55 + 40 * value };
        return (
            component(value / 36),
            component((value / 6) % 6),
            component(value % 6),
        );
    }
    let gray = 8 + 10 * (index.saturating_sub(232));
    (gray, gray, gray)
}
