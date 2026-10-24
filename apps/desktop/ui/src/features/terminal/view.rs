use super::TerminalFeature;
use crate::{
    components::{ScrollAxis, action_button, scroll_viewport},
    theme,
};
use gpui::{Context, Pixels, Render, ScrollHandle, Window, canvas, div, prelude::*, px};
use lapis_terminal::{TerminalCell, TerminalColor, TerminalId, TerminalRow, TerminalShell};

impl Render for TerminalFeature {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_id();
        // visible_outputの更新に必要な値だけを先に取得
        let screen_for_cursor = self.active_screen().map(|s| {
            let mut cursor = s.cursor;
            cursor.row = cursor
                .row
                .saturating_add(u16::try_from(s.scrollback.len()).unwrap_or(u16::MAX));
            cursor
        });
        let total_rows = self
            .active_screen()
            .map(|s| s.scrollback.len() + s.rows.len())
            .unwrap_or(0);

        // visible_outputの更新とcursor-follow scroll
        let output = self
            .terminals
            .iter()
            .find(|terminal| Some(&terminal.id) == active.as_ref())
            .map(|terminal| (terminal.id.clone(), terminal.revision));

        let handle = self.scroll.handle();
        if follow_output(
            &mut self.visible_output,
            output,
            self.focus.is_focused(window),
            screen_for_cursor.map(|cursor| cursor.row),
            &handle,
            handle.bounds().size.height,
        ) {
            // 今frameのlayoutを待ち、表示可能になったときだけ次のrenderを起こす。
            // 高さ0の非表示Panelではnotifyを連鎖させず、pendingを維持する。
            cx.on_next_frame(window, |feature, _, cx| {
                if retry_after_layout(
                    &feature.visible_output,
                    feature.scroll.handle().bounds().size.height,
                ) {
                    cx.notify();
                }
            });
        }

        // 参照で読み、visible rowsだけcloneする
        let screen_ref = self.active_screen();

        // virtualization: 表示中の行のみを処理
        let row_height = 16.0_f32;

        let visible_range = if total_rows > 0 {
            let handle = self.scroll.handle();
            let height = handle.bounds().size.height;
            let offset = handle.offset();

            if height > px(0.0) {
                let visible_rows = (height.to_f64() / 16.0).ceil() as usize;
                let first_visible = ((-offset.y).to_f64() / 16.0).floor() as usize;
                // overscanを2行に限定
                let overscan = 2;
                let last_visible = (first_visible + visible_rows + overscan).min(total_rows);
                first_visible..last_visible
            } else {
                // height未確定時はvisible screen相当 + overscanのみ
                let fallback_visible = screen_ref.map(|s| s.rows.len()).unwrap_or(24);
                (total_rows.saturating_sub(fallback_visible))..total_rows
            }
        } else {
            0..0
        };

        // top/bottom spacerの高さを計算
        let top_spacer_height = px(visible_range.start as f32 * row_height);
        let bottom_spacer_height = if visible_range.end < total_rows {
            px((total_rows - visible_range.end) as f32 * row_height)
        } else {
            px(0.0)
        };

        // 表示行のみを収集
        let rows: Vec<TerminalRow> = screen_ref
            .map(|screen| {
                let scrollback_len = screen.scrollback.len();
                let mut rows = Vec::with_capacity(visible_range.len());

                for i in visible_range.clone() {
                    if i < scrollback_len {
                        rows.push(screen.scrollback[i].clone());
                    } else if i >= scrollback_len && i < scrollback_len + screen.rows.len() {
                        rows.push(screen.rows[i - scrollback_len].clone());
                    }
                }
                rows
            })
            .unwrap_or_default();

        let cursor = screen_for_cursor;
        let entity = cx.entity();
        let focus = self.focus.clone();
        let tabs = self
            .terminals
            .iter()
            .map(|terminal| (terminal.id.clone(), terminal.status))
            .collect::<Vec<_>>();
        let discovered = self.discovered.clone();
        // clipboard用：ボタン押下時にだけplain_textを生成する
        let screen_for_clipboard = screen_ref.map(|s| s.plain_text());
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
                            if let Some(text) = &screen_for_clipboard {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                    text.clone(),
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
                        // top spacer: 見えていない上部行の高さを確保
                        .when(top_spacer_height > px(0.0), |el| {
                            el.child(div().h(top_spacer_height))
                        })
                        // visible rowsのみ描画
                        .children(rows.into_iter().map(render_row))
                        // bottom spacer: 見えていない下部行の高さを確保
                        .when(bottom_spacer_height > px(0.0), |el| {
                            el.child(div().h(bottom_spacer_height))
                        })
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
                                        // cursor位置はspacer分を考慮
                                        let cursor_y = px(f32::from(cursor.row) * 16.0);
                                        window.paint_quad(gpui::fill(
                                            gpui::Bounds::new(
                                                bounds.origin
                                                    + gpui::point(
                                                        px(f32::from(cursor.column) * 7.8),
                                                        cursor_y,
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

/// trueのとき、呼出側はlayout後のframe callbackを予約する。
fn follow_output(
    visible_output: &mut Option<Option<(TerminalId, u64)>>,
    output: Option<(TerminalId, u64)>,
    focused: bool,
    cursor_row: Option<u16>,
    handle: &ScrollHandle,
    height: Pixels,
) -> bool {
    let Some(output) = output else {
        *visible_output = None;
        return false;
    };
    if visible_output.as_ref().and_then(Option::as_ref) == Some(&output) {
        return false;
    }
    *visible_output = Some(None);
    if focused && let Some(row) = cursor_row {
        if height <= px(0.0) {
            return true;
        }
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
    *visible_output = Some(Some(output));
    false
}

fn retry_after_layout(visible_output: &Option<Option<(TerminalId, u64)>>, height: Pixels) -> bool {
    matches!(visible_output, Some(None)) && height > px(0.0)
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
    if cell.dim {
        foreground = dim_color(foreground);
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

fn dim_color(color: gpui::Rgba) -> gpui::Rgba {
    gpui::Rgba {
        r: color.r * 0.66,
        g: color.g * 0.66,
        b: color.b * 0.66,
        a: color.a,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn output(id: &str, revision: u64) -> Option<(TerminalId, u64)> {
        Some((TerminalId::new(id), revision))
    }

    #[test]
    fn scroll_follow_reserves_retry_then_finishes_with_unchanged_output() {
        let mut state = None;
        let handle = ScrollHandle::new();
        assert!(follow_output(
            &mut state,
            output("first", 1),
            true,
            Some(120),
            &handle,
            px(0.0)
        ));
        assert_eq!(state, Some(None));
        // 0のままならnotifyしない。layoutで高さが得られたら次frameを描画する。
        assert!(!retry_after_layout(&state, px(0.0)));
        assert!(retry_after_layout(&state, px(320.0)));
        assert!(!follow_output(
            &mut state,
            output("first", 1),
            true,
            Some(120),
            &handle,
            px(320.0)
        ));
        assert_eq!(handle.offset().y, px(320.0 - 121.0 * 16.0));
        assert_eq!(state, Some(output("first", 1)));
        assert!(!retry_after_layout(&state, px(320.0)));
    }

    #[test]
    fn scroll_follow_with_layout_scrolls_immediately_and_only_once() {
        let mut state = None;
        let handle = ScrollHandle::new();
        assert!(!follow_output(
            &mut state,
            output("first", 1),
            true,
            Some(120),
            &handle,
            px(320.0)
        ));
        assert_eq!(state, Some(output("first", 1)));
        assert_eq!(handle.offset().y, px(-1616.0));
        handle.set_offset(gpui::point(px(0.0), px(0.0)));
        assert!(!follow_output(
            &mut state,
            output("first", 1),
            true,
            Some(120),
            &handle,
            px(320.0)
        ));
        assert_eq!(handle.offset().y, px(0.0));
    }

    #[test]
    fn scroll_follow_completes_pending_without_cursor_or_focus() {
        for (focused, cursor) in [(true, None), (false, Some(120))] {
            let mut state = Some(None);
            assert!(!follow_output(
                &mut state,
                output("first", 1),
                focused,
                cursor,
                &ScrollHandle::new(),
                px(0.0)
            ));
            assert_eq!(state, Some(output("first", 1)));
        }
    }

    #[test]
    fn scroll_follow_retries_after_terminal_switch_and_new_snapshot_revision() {
        let handle = ScrollHandle::new();
        let mut state = Some(output("first", 1));
        // tab/Task切替、元Terminalへの復帰、Attach/reconnect/resyncのrevision更新。
        for next in [output("second", 1), output("first", 1), output("first", 2)] {
            assert!(follow_output(
                &mut state,
                next.clone(),
                true,
                Some(120),
                &handle,
                px(0.0)
            ));
            assert!(retry_after_layout(&state, px(320.0)));
            assert!(!follow_output(
                &mut state,
                next.clone(),
                true,
                Some(120),
                &handle,
                px(320.0)
            ));
            assert_eq!(state, Some(next));
        }
    }
}
