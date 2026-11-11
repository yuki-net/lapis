use super::TerminalFeature;
use super::canvas::{TerminalColors, TerminalElement};
use crate::{
    components::{Menu, MenuEntry, MenuItemSpec, ScrollAxis, scroll_viewport},
    theme,
};
use gpui::{
    App, Context, Pixels, Render, ScrollHandle, Window, canvas, div, point, prelude::*, px,
};
use terminal::{TerminalCursor, TerminalId};

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
        // この frame の描画で出力の変更を消費したことにする。
        self.pending_output_render = false;
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
                let fallback_visible = self.active_screen().map(|s| s.rows.len()).unwrap_or(24);
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

        let cursor = screen_for_cursor;
        let caret_shape = self.caret_shape;
        let entity = cx.entity();
        let colors = theme::colors(cx);
        let terminal_element = TerminalElement {
            feature: entity.clone(),
            metrics: metrics.clone(),
            range: visible_range.clone(),
            colors: TerminalColors {
                foreground: colors.text_primary,
                background: colors.background_primary,
            },
        };
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
        // パネル内のタブ列は持たない。通常は1セッションだけを表示し、
        // 複数あるときだけ1つのボタンから一覧を開く。
        let session_count = self.terminals.len();
        let active_status = active.as_ref().and_then(|id| {
            self.terminals
                .iter()
                .find(|terminal| &terminal.id == id)
                .map(|terminal| terminal.status)
        });
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
                    .child(
                        div()
                            .px_2()
                            .text_size(px(11.0))
                            .text_color(theme::colors(cx).text_secondary)
                            .child(match active_status {
                                Some(status) => status_label(status).to_owned(),
                                None => "ターミナル".to_owned(),
                            }),
                    )
                    .when(session_count > 1, |el| {
                        el.child(
                            header_chip(
                                "terminal-sessions",
                                format!("{session_count} セッション"),
                                cx,
                            )
                            .debug_selector(|| "terminal-sessions".into())
                            .on_click(cx.listener(
                                |this, event: &gpui::ClickEvent, window, cx| {
                                    this.open_menu(
                                        event
                                            .mouse_position()
                                            .map(|position| point(position.x, position.y)),
                                        cx,
                                    );
                                    crate::gpui_compat::focus(window, &this.focus, cx);
                                },
                            )),
                        )
                    })
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
                        .child(terminal_element)
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
        // 複数セッションがあるときだけ、切替えの最小UIとして一覧を出す。
        // タブ列は持たず、番号付きの一覧から1つを選ぶ。
        if self.terminals.len() > 1 {
            let active = self.active_id();
            for (index, terminal) in self.terminals.iter().enumerate() {
                let id = terminal.id.clone();
                let label = format!(
                    "{}. {} ({})",
                    index + 1,
                    terminal.id.as_str(),
                    status_label(terminal.status)
                );
                let menu_id =
                    gpui::SharedString::from(format!("terminal.session.{}", terminal.id.as_str()));
                let listener = cx.listener(
                    move |this: &mut TerminalFeature,
                          _: &gpui::ClickEvent,
                          window: &mut Window,
                          cx: &mut Context<TerminalFeature>| {
                        this.context_menu = None;
                        this.select_terminal(id.clone(), cx);
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
                        selected: active.as_ref() == Some(&terminal.id),
                        indent: 0,
                        tree: None,
                        submenu: false,
                        separator_before: false,
                    },
                    cx,
                    move |row| row.on_click(listener).into_any_element(),
                ));
            }
        }
        entries.push(menu_entry(
            "terminal.menu.new",
            "新しいシェル",
            true,
            !self.terminals.is_empty(),
            cx,
            |this, window, cx| {
                this.start(cx);
                crate::gpui_compat::focus(window, &this.focus, cx);
            },
        ));
        // 特定の profile で始める操作は例外として残す。標準フローは自動起動。
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
                    selected: self.resolved_default_profile().as_ref() == Some(&profile.id),
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

/// セッション数など、パネルヘッダーの状態表示に使う小さなチップ。
fn header_chip(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
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
        .text_color(colors.text_secondary)
        .hover(|style| style.bg(colors.button_background_hover))
        .child(label.into())
}

/// セッション状態の日本語ラベル。Debug 表記をUIへ出さない。
fn status_label(status: terminal::TerminalStatus) -> &'static str {
    match status {
        terminal::TerminalStatus::Running => "実行中",
        terminal::TerminalStatus::Disconnected => "未接続",
        terminal::TerminalStatus::Exited => "終了",
        terminal::TerminalStatus::Failed => "失敗",
    }
}

/// ターミナルの描画に使う寸法。フォント計測から求める。
#[derive(Clone)]
pub(super) struct TerminalMetrics {
    pub cell_width: Pixels,
    pub row_height: Pixels,
    pub font_family: gpui::SharedString,
    pub font_size: Pixels,
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
            font_size,
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

#[cfg(test)]
mod tests {
    use super::*;
    use terminal::TerminalStatus;

    #[test]
    fn status_labels_are_readable_strings_not_debug_names() {
        for (status, label) in [
            (TerminalStatus::Running, "実行中"),
            (TerminalStatus::Disconnected, "未接続"),
            (TerminalStatus::Exited, "終了"),
            (TerminalStatus::Failed, "失敗"),
        ] {
            assert_eq!(status_label(status), label);
        }
    }

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
