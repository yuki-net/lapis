use super::TerminalFeature;
use crate::{
    components::{Menu, MenuEntry, MenuItemSpec, ScrollAxis, scroll_viewport},
    theme,
};
use gpui::{
    App, Context, Pixels, Render, ScrollHandle, Window, canvas, div, point, prelude::*, px,
};
use terminal::{TerminalCell, TerminalColor, TerminalCursor, TerminalId, TerminalRow};

/// セル幅を決めるサンプル文字。
const CELL_SAMPLE: char = 'M';
/// レイアウト幅から列数を決めるときの最小列数。
const MIN_COLUMNS: u16 = 20;
/// レイアウト幅から行数を決めるときの最小行数。
const MIN_ROWS: u16 = 5;
/// 画面外の行を余分に描画してスクロールのちらつきを抑える。
const OVERSCAN_ROWS: usize = 2;

impl Render for TerminalFeature {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_id();
        let metrics = self.metrics(window);
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
            metrics.row_height,
        ) {
            // 今frameのlayoutを待ち、表示可能になったときだけ次のrenderを起こす。
            cx.on_next_frame(window, |feature, _, cx| {
                if retry_after_layout(
                    &feature.visible_output,
                    feature.scroll.handle().bounds().size.height,
                ) {
                    cx.notify();
                }
            });
        }

        let screen_ref = self.active_screen();

        let visible_range = if total_rows > 0 {
            let handle = self.scroll.handle();
            let height = handle.bounds().size.height;
            let offset = handle.offset();

            if height > px(0.0) {
                let visible_rows = (height.to_f64() / metrics.row_height.to_f64()).ceil() as usize;
                let first_visible =
                    ((-offset.y).to_f64() / metrics.row_height.to_f64()).floor() as usize;
                visible_row_range(total_rows, visible_rows, first_visible)
            } else {
                let fallback_visible = screen_ref.map(|s| s.rows.len()).unwrap_or(24);
                (total_rows.saturating_sub(fallback_visible))..total_rows
            }
        } else {
            0..0
        };

        let top_spacer_height = px(visible_range.start as f32 * metrics.row_height.to_f64() as f32);
        let bottom_spacer_height = if visible_range.end < total_rows {
            px((total_rows - visible_range.end) as f32 * metrics.row_height.to_f64() as f32)
        } else {
            px(0.0)
        };

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
        let caret_shape = self.caret_shape;
        let entity = cx.entity();
        let focus = self.focus.clone();
        // 表示領域に合わせて PTY のサイズを追従させる。layout 確定後の次フレームで反映する。
        self.last_metrics = Some(metrics.clone());
        let viewport = self.scroll.handle().bounds().size;
        if let Some(size) = self.desired_size(viewport)
            && let Some(id) = self.active_id()
            && self.last_size.as_ref() != Some(&(id, size))
        {
            cx.on_next_frame(window, move |feature, _, cx| {
                feature.fit_to_viewport(viewport, cx);
            });
        }
        let cell_width = metrics.cell_width;
        let row_height = metrics.row_height;
        let owner = active
            .as_ref()
            .and_then(|id| self.owners.get(id.as_str()))
            .cloned()
            .flatten();
        let tabs = self
            .terminals
            .iter()
            .map(|terminal| (terminal.id.clone(), terminal.status))
            .collect::<Vec<_>>();
        let discovered = self.discovered.clone();
        let context_open = self.context_menu;
        let context_anchor = self.context_anchor;
        let fonts = metrics.font_family.clone();
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
                    .items_center()
                    .gap_1()
                    .px_1()
                    .border_b_1()
                    .border_color(theme::colors(cx).border_default)
                    .children(tabs.into_iter().enumerate().map(|(index, (id, status))| {
                        let selected = active.as_ref() == Some(&id);
                        let tab_id = gpui::ElementId::named_usize("terminal-tab", index);
                        tab_button(tab_id, format!("{} − {status:?}", index + 1), selected, cx)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select_terminal(id.clone(), cx);
                                crate::gpui_compat::focus(window, &this.focus, cx);
                            }))
                    }))
                    .child(
                        tab_button("terminal-new-tab", "+", false, cx)
                            .debug_selector(|| "terminal-new-tab".into())
                            .on_click(cx.listener(|this, event: &gpui::ClickEvent, window, cx| {
                                this.open_menu(
                                    event
                                        .mouse_position()
                                        .map(|position| point(position.x, position.y)),
                                    cx,
                                );
                                crate::gpui_compat::focus(window, &this.focus, cx);
                            })),
                    )
                    .child(div().flex_1())
                    .when_some(owner, |el, owner| {
                        el.child(
                            div()
                                .text_size(px(10.0))
                                .text_color(theme::colors(cx).text_warning)
                                .child(format!("操作中: {owner}")),
                        )
                    })
                    .child(
                        div()
                            .text_size(px(10.0))
                            .text_color(theme::colors(cx).text_tertiary)
                            .child(self.message.clone()),
                    ),
            )
            .child(
                scroll_viewport(
                    "terminal-output",
                    ScrollAxis::Both,
                    &self.scroll,
                    div()
                        .relative()
                        .size_full()
                        .font_family(fonts)
                        .text_size(px(13.0))
                        .line_height(row_height)
                        .text_color(theme::colors(cx).text_primary)
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                crate::gpui_compat::focus(window, &this.focus, cx)
                            }),
                        )
                        .on_mouse_down(
                            gpui::MouseButton::Right,
                            cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                                this.open_menu(Some(point(event.position.x, event.position.y)), cx);
                            }),
                        )
                        .when(top_spacer_height > px(0.0), |el| {
                            el.child(div().h(top_spacer_height))
                        })
                        .children(
                            rows.into_iter()
                                .map(|row| render_row(row, cell_width, row_height, cx)),
                        )
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
                                        window.paint_quad(gpui::fill(
                                            caret_bounds(
                                                bounds.origin,
                                                cursor,
                                                cell_width,
                                                row_height,
                                                caret_shape,
                                            ),
                                            theme::colors(cx).editor_caret,
                                        ));
                                    }
                                },
                            )
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full(),
                        ),
                    cx,
                )
                .flex_1(),
            )
            .when(!self.marked.is_empty(), |view| {
                view.child(self.marked.clone())
            })
            .when_some(
                context_open.filter(|_| context_anchor.is_some()),
                |view, _| {
                    let anchor = context_anchor.unwrap_or_else(|| point(px(0.0), px(0.0)));
                    view.child(self.render_context_menu(anchor, discovered, cx))
                },
            )
    }
}

impl TerminalFeature {
    fn render_context_menu(
        &self,
        anchor: gpui::Point<Pixels>,
        discovered: Vec<terminal::TerminalSnapshot>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut entries = Vec::new();
        entries.push(menu_entry(
            "terminal.menu.new",
            "新しいシェル",
            true,
            false,
            cx,
            |this, window, cx| {
                this.start(cx);
                crate::gpui_compat::focus(window, &this.focus, cx);
            },
        ));
        // 起動できる profile は Host が決める。Web も接続先 Host の一覧を使う。
        for profile in &self.profiles.profiles {
            let id = profile.id.clone();
            let label = format!("{} を開く", profile.name);
            let menu_id =
                gpui::SharedString::from(format!("terminal.profile.{}", profile.id.as_str()));
            let listener = cx.listener(
                move |this: &mut TerminalFeature,
                      _: &gpui::ClickEvent,
                      window: &mut Window,
                      cx: &mut Context<TerminalFeature>| {
                    this.context_menu = None;
                    this.start_profile(Some(id.clone()), cx);
                    crate::gpui_compat::focus(window, &this.focus, cx);
                    cx.notify();
                },
            );
            entries.push(MenuEntry::new(
                MenuItemSpec {
                    id: menu_id,
                    label: label.into(),
                    shortcut: None,
                    icon: None,
                    enabled: true,
                    selected: self.profiles.default.as_ref() == Some(&profile.id),
                    indent: 1,
                    tree: None,
                    submenu: false,
                    separator_before: false,
                },
                cx,
                move |row| row.on_click(listener).into_any_element(),
            ));
        }
        entries.push(menu_entry(
            "terminal.menu.paste",
            "貼り付け",
            true,
            true,
            cx,
            |this, window, cx| {
                this.send_clipboard(cx);
                crate::gpui_compat::focus(window, &this.focus, cx);
            },
        ));
        entries.push(menu_entry(
            "terminal.menu.claim",
            "操作権を取得",
            true,
            true,
            cx,
            |this, _, cx| this.claim(cx),
        ));
        entries.push(menu_entry(
            "terminal.menu.release",
            "操作権を手放す",
            true,
            false,
            cx,
            |this, _, cx| this.release(cx),
        ));
        entries.push(menu_entry(
            "terminal.menu.copy",
            "画面をコピー",
            true,
            false,
            cx,
            |this, _, cx| {
                if let Some(screen) = this.active_screen() {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(screen.plain_text()));
                }
            },
        ));
        entries.push(menu_entry_inner(
            "terminal.menu.discover",
            "Host の端末を探す",
            true,
            true,
            false,
            cx,
            |this, _, cx| this.discover(cx),
        ));
        for terminal in discovered {
            let label = format!("接続: {} ({:?})", terminal.id.as_str(), terminal.status);
            let id = terminal.id.clone();
            let listener = cx.listener(
                move |this: &mut TerminalFeature,
                      _: &gpui::ClickEvent,
                      window: &mut Window,
                      cx: &mut Context<TerminalFeature>| {
                    this.attach(id.clone(), cx);
                    this.context_menu = None;
                    crate::gpui_compat::focus(window, &this.focus, cx);
                },
            );
            entries.push(MenuEntry::new(
                MenuItemSpec {
                    id: format!("terminal.attach.{}", terminal.id.as_str()).into(),
                    label: label.into(),
                    shortcut: None,
                    icon: None,
                    enabled: true,
                    selected: false,
                    indent: 1,
                    tree: None,
                    submenu: false,
                    separator_before: false,
                },
                cx,
                move |row| row.on_click(listener).into_any_element(),
            ));
        }
        entries.push(menu_entry(
            "terminal.menu.terminate",
            "終了",
            true,
            true,
            cx,
            |this, window, cx| this.confirm_terminate(window, cx),
        ));
        crate::components::floating_tree(anchor, point(px(0.0), px(0.0))).child(
            div()
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.context_menu = None;
                    cx.notify();
                }))
                .child(
                    Menu::new("terminal-context-menu", entries)
                        .debug_selector("terminal-context-menu"),
                ),
        )
    }
}

fn menu_entry(
    id: &'static str,
    label: &'static str,
    enabled: bool,
    separator_before: bool,
    cx: &mut Context<TerminalFeature>,
    handler: impl Fn(&mut TerminalFeature, &mut Window, &mut Context<TerminalFeature>) + 'static,
) -> MenuEntry {
    menu_entry_inner(id, label, enabled, separator_before, true, cx, handler)
}

/// `close_on_click` が false の項目は、結果を同じメニューで見せたいときに使う。
#[allow(clippy::too_many_arguments)]
fn menu_entry_inner(
    id: &'static str,
    label: &'static str,
    enabled: bool,
    separator_before: bool,
    close_on_click: bool,
    cx: &mut Context<TerminalFeature>,
    handler: impl Fn(&mut TerminalFeature, &mut Window, &mut Context<TerminalFeature>) + 'static,
) -> MenuEntry {
    let listener = cx.listener(
        move |this: &mut TerminalFeature,
              _: &gpui::ClickEvent,
              window: &mut Window,
              cx: &mut Context<TerminalFeature>| {
            if close_on_click {
                this.context_menu = None;
            }
            handler(this, window, cx);
            cx.notify();
        },
    );
    MenuEntry::new(
        MenuItemSpec {
            id: id.into(),
            label: label.into(),
            shortcut: None,
            icon: None,
            enabled,
            selected: false,
            indent: 0,
            tree: None,
            submenu: false,
            separator_before,
        },
        cx,
        move |row| {
            row.when(enabled, |row| row.on_click(listener))
                .into_any_element()
        },
    )
}

fn tab_button(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
    selected: bool,
    cx: &App,
) -> gpui::Stateful<gpui::Div> {
    let colors = theme::colors(cx);
    div()
        .id(id)
        .flex()
        .items_center()
        .px_2()
        .py_0p5()
        .rounded(px(4.0))
        .text_size(px(11.0))
        .text_color(if selected {
            colors.text_primary
        } else {
            colors.text_secondary
        })
        .when(selected, |button| {
            button.bg(colors.button_background_selected)
        })
        .child(label.into())
}

/// ターミナルの描画に使う寸法。フォント計測から求める。
#[derive(Clone)]
pub(super) struct TerminalMetrics {
    pub cell_width: Pixels,
    pub row_height: Pixels,
    pub font_family: gpui::SharedString,
}

impl TerminalFeature {
    /// フォント計測からセル幅・行高を求める。失敗時は既定値にフォールバックする。
    pub(super) fn metrics(&self, window: &Window) -> TerminalMetrics {
        let font_family = self.font_family.clone();
        let font_size = px(self.font_size);
        let font_id = window
            .text_system()
            .resolve_font(&gpui::font(font_family.clone()));
        let cell_width = window
            .text_system()
            .advance(font_id, font_size, CELL_SAMPLE)
            .map(|size| size.width)
            .unwrap_or(px(self.font_size * 0.6));
        TerminalMetrics {
            cell_width,
            row_height: px(self.font_size * f32::from(self.line_height_percent) / 100.0),
            font_family,
        }
    }

    /// 表示領域の大きさから列数・行数を求める。0 のときは未確定。
    pub(super) fn desired_size(&self, size: gpui::Size<Pixels>) -> Option<(u16, u16)> {
        let metrics = self.last_metrics.as_ref()?;
        if size.width <= px(0.0) || size.height <= px(0.0) {
            return None;
        }
        let columns = (size.width.to_f64() / metrics.cell_width.to_f64()).floor() as u16;
        let rows = (size.height.to_f64() / metrics.row_height.to_f64()).floor() as u16;
        Some((columns.max(MIN_COLUMNS), rows.max(MIN_ROWS)))
    }

    /// 表示領域の大きさに合わせて PTY のサイズを更新する。
    pub(super) fn fit_to_viewport(&mut self, size: gpui::Size<Pixels>, cx: &mut Context<Self>) {
        let Some((columns, rows)) = self.desired_size(size) else {
            return;
        };
        let Some(id) = self.active_id() else {
            return;
        };
        // Terminal が変われば同じ寸法でも適用し直す必要がある。
        if self.last_size.as_ref() == Some(&(id.clone(), (columns, rows))) {
            return;
        }
        self.last_size = Some((id, (columns, rows)));
        self.resize(columns, rows, cx);
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
    row_height: Pixels,
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
        let top = px(f32::from(row) * row_height.to_f64() as f32);
        let bottom = top + row_height;
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

fn visible_row_range(
    total_rows: usize,
    visible_rows: usize,
    calculated_first: usize,
) -> std::ops::Range<usize> {
    let max_first = total_rows.saturating_sub(visible_rows);
    let first = calculated_first.min(max_first);
    let last = first
        .saturating_add(visible_rows)
        .saturating_add(OVERSCAN_ROWS)
        .min(total_rows);
    first..last
}

/// カーソル形状ごとの描画範囲。bar は幅、underline は下端の高さを変える。
fn caret_bounds(
    origin: gpui::Point<Pixels>,
    cursor: TerminalCursor,
    cell_width: Pixels,
    row_height: Pixels,
    shape: settings::TerminalCaretShape,
) -> gpui::Bounds<Pixels> {
    let x = px(origin.x.to_f64() as f32 + f32::from(cursor.column) * cell_width.to_f64() as f32);
    let y = px(origin.y.to_f64() as f32 + f32::from(cursor.row) * row_height.to_f64() as f32);
    let (width, height, y) = match shape {
        settings::TerminalCaretShape::Block => (cell_width, row_height, y),
        settings::TerminalCaretShape::Bar => (
            px(f32::max(1.0, cell_width.to_f64() as f32 * 0.12)),
            row_height,
            y,
        ),
        settings::TerminalCaretShape::Underline => (
            cell_width,
            px(f32::max(1.0, row_height.to_f64() as f32 * 0.1)),
            px(y.to_f64() as f32 + row_height.to_f64() as f32
                - f32::max(1.0, row_height.to_f64() as f32 * 0.1)),
        ),
    };
    gpui::Bounds::new(gpui::point(x, y), gpui::size(width, height))
}

fn render_row(
    row: TerminalRow,
    cell_width: Pixels,
    row_height: Pixels,
    cx: &gpui::App,
) -> impl IntoElement {
    div().flex().h(row_height).children(
        row.cells
            .into_iter()
            .map(|cell| render_cell(cell, cell_width, row_height, cx)),
    )
}

fn render_cell(
    cell: TerminalCell,
    cell_width: Pixels,
    row_height: Pixels,
    cx: &gpui::App,
) -> impl IntoElement {
    let default_foreground = theme::colors(cx).text_primary;
    let mut foreground = color(cell.foreground, default_foreground);
    // 既定の背景は透明にし、パネルの背景を見せる。
    let mut background = match cell.background {
        TerminalColor::Default => None,
        explicit => Some(color(explicit, default_foreground)),
    };
    if cell.inverse {
        let swapped = background.unwrap_or_else(|| theme::colors(cx).background_primary);
        background = Some(foreground);
        foreground = swapped;
    }
    if cell.dim {
        foreground = dim_color(foreground);
    }
    let mut view = div()
        .flex_none()
        .w(px(cell_width.to_f64() as f32 * f32::from(cell.width)))
        .h(row_height)
        .whitespace_nowrap()
        .text_color(foreground)
        .when_some(background, |view, background| view.bg(background))
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

    #[test]
    fn visible_row_range_clamps_old_offsets_and_keeps_overscan() {
        for (total, visible, first, expected) in [
            (100, 20, 10, 10..32),
            (20, 10, 1_000, 10..20),
            (5, 20, 1_000, 0..5),
            (0, 20, 1_000, 0..0),
        ] {
            assert_eq!(visible_row_range(total, visible, first), expected);
        }
    }

    #[test]
    fn caret_shape_changes_only_the_painted_rectangle() {
        let cursor = TerminalCursor {
            row: 2,
            column: 3,
            visible: true,
        };
        let origin = gpui::point(px(10.0), px(20.0));
        let cell_width = px(8.0);
        let row_height = px(20.0);

        let block = caret_bounds(
            origin,
            cursor,
            cell_width,
            row_height,
            settings::TerminalCaretShape::Block,
        );
        let bar = caret_bounds(
            origin,
            cursor,
            cell_width,
            row_height,
            settings::TerminalCaretShape::Bar,
        );
        let underline = caret_bounds(
            origin,
            cursor,
            cell_width,
            row_height,
            settings::TerminalCaretShape::Underline,
        );

        assert_eq!(block.origin, gpui::point(px(34.0), px(60.0)));
        assert_eq!(block.size, gpui::size(cell_width, row_height));
        assert!(bar.size.width < cell_width);
        assert_eq!(bar.size.height, row_height);
        assert_eq!(underline.size.width, cell_width);
        assert!(underline.size.height < row_height);
        assert_eq!(
            underline.origin.y + underline.size.height,
            block.origin.y + row_height
        );
    }

    fn output(id: &str, revision: u64) -> Option<(TerminalId, u64)> {
        Some((TerminalId::new(id), revision))
    }

    #[test]
    fn scroll_follow_reserves_retry_then_finishes_with_unchanged_output() {
        let mut state = None;
        let handle = ScrollHandle::new();
        let row_height = px(16.0);
        assert!(follow_output(
            &mut state,
            output("first", 1),
            true,
            Some(120),
            &handle,
            px(0.0),
            row_height
        ));
        assert_eq!(state, Some(None));
        assert!(!retry_after_layout(&state, px(0.0)));
        assert!(retry_after_layout(&state, px(320.0)));
        assert!(!follow_output(
            &mut state,
            output("first", 1),
            true,
            Some(120),
            &handle,
            px(320.0),
            row_height
        ));
        assert_eq!(handle.offset().y, px(320.0 - 121.0 * 16.0));
        assert_eq!(state, Some(output("first", 1)));
        assert!(!retry_after_layout(&state, px(320.0)));
    }

    #[test]
    fn scroll_follow_with_layout_scrolls_immediately_and_only_once() {
        let mut state = None;
        let handle = ScrollHandle::new();
        let row_height = px(16.0);
        assert!(!follow_output(
            &mut state,
            output("first", 1),
            true,
            Some(120),
            &handle,
            px(320.0),
            row_height
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
            px(320.0),
            row_height
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
                px(0.0),
                px(16.0)
            ));
            assert_eq!(state, Some(output("first", 1)));
        }
    }

    #[test]
    fn scroll_follow_retries_after_terminal_switch_and_new_snapshot_revision() {
        let handle = ScrollHandle::new();
        let row_height = px(16.0);
        let mut state = Some(output("first", 1));
        for next in [output("second", 1), output("first", 1), output("first", 2)] {
            assert!(follow_output(
                &mut state,
                next.clone(),
                true,
                Some(120),
                &handle,
                px(0.0),
                row_height
            ));
            assert!(retry_after_layout(&state, px(320.0)));
            assert!(!follow_output(
                &mut state,
                next.clone(),
                true,
                Some(120),
                &handle,
                px(320.0),
                row_height
            ));
            assert_eq!(state, Some(next));
        }
    }
}
