use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{AnyElement, Context, Div, Inspector, IntoElement, Window, div, prelude::*, px};

use crate::{
    components::{
        ScrollAxis, ScrollState, header_button, panel_header, panel_surface, scroll_surface,
        scroll_viewport,
    },
    theme, tokens,
};

use super::console::entry::{DiagnosticLevel, DiagnosticLogEntry};
use super::console::store::store;
use super::elements::tree_view::{render_active_summary, render_tree};
use super::inspector_controller::InspectorController;
use super::state::snapshot::{StateEntry, StateNode};
use super::tool::DevToolTab;

pub(super) fn render_inspector(
    inspector: &mut Inspector,
    window: &mut Window,
    cx: &mut Context<Inspector>,
) -> AnyElement {
    let entity = cx.entity_id();
    let active_tab = InspectorController::active_tab(entity, cx);
    let content_scroll = InspectorController::content_scroll(entity, cx);

    // DevToolタブバー
    let tab_bar = div()
        .id("devtool-tab-bar")
        .flex()
        .items_center()
        .px(tokens::spacing::SM)
        .py(px(4.))
        .gap(px(2.))
        .bg(theme::colors(cx).background_secondary)
        .border_b_1()
        .border_color(theme::colors(cx).border_default)
        .children(DevToolTab::all().iter().enumerate().map(|(index, tab)| {
            let is_active = *tab == active_tab;
            let tab_label = tab.title();
            div()
                .id(("devtool-tab", index))
                .px(tokens::spacing::MD)
                .py(px(4.))
                .rounded(px(4.))
                .cursor_pointer()
                .text_size(px(12.))
                .font_weight(if is_active {
                    gpui::FontWeight::SEMIBOLD
                } else {
                    gpui::FontWeight::NORMAL
                })
                .when(is_active, |el| {
                    el.bg(theme::colors(cx).button_background_selected)
                        .text_color(theme::colors(cx).text_primary)
                })
                .when(!is_active, |el| {
                    el.text_color(theme::colors(cx).text_secondary)
                })
                .when(!is_active, |el| {
                    el.bg(theme::colors(cx).button_background_hover)
                })
                .child(tab_label)
                .on_click(cx.listener(move |_, _, _, cx| {
                    InspectorController::set_active_tab(*tab, entity, cx);
                    cx.notify();
                }))
        }));

    // タブコンテンツ
    let tab_content = match active_tab {
        DevToolTab::Elements => render_elements_tab(inspector, window, cx, &content_scroll),
        DevToolTab::Console => render_console_tab(inspector, window, cx),
        DevToolTab::Network => render_network_tab(cx),
        DevToolTab::Performance => render_performance_tab(cx),
        DevToolTab::State => render_state_tab(inspector, window, cx),
    };

    div()
        .size_full()
        .flex()
        .flex_col()
        .bg(theme::colors(cx).background_primary)
        .text_color(theme::colors(cx).text_primary)
        .child(crate::window_host::chrome::title_bar(
            div()
                .id("inspector-title")
                .h_full()
                .flex_1()
                .flex()
                .items_center()
                .px_3()
                .child("Lapis DevTool"),
            |_, window, _| window.remove_window(),
            cx,
        ))
        .child(tab_bar)
        .child(
            div()
                .id("devtool-content")
                .min_h(px(0.))
                .flex_1()
                .child(tab_content),
        )
        .into_any_element()
}

fn render_elements_tab(
    inspector: &mut Inspector,
    window: &mut Window,
    cx: &mut Context<Inspector>,
    content_scroll: &ScrollState,
) -> Div {
    let details_header = panel_header()
        .id("inspector-details-header")
        .px(tokens::spacing::SM)
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .child("Element details"),
        )
        .child(
            header_button("inspector-refresh", "Refresh", cx)
                .cursor_pointer()
                .on_click(cx.listener(|_, _, _, cx| {
                    InspectorController::defer(cx, InspectorController::refresh_target);
                })),
        )
        .child(
            header_button("inspector-pick", "Pick element", cx)
                .cursor_pointer()
                .when(inspector.is_picking(), |button| {
                    button.bg(theme::colors(cx).button_background_selected)
                })
                .on_click(cx.listener(|inspector, _, window, cx| {
                    inspector.start_picking();
                    window.refresh();
                    cx.notify();
                    InspectorController::defer(cx, InspectorController::repaint_target);
                })),
        );
    div()
        .size_full()
        .flex()
        .p(tokens::spacing::GAP)
        .gap(tokens::spacing::GAP)
        .child(
            panel_surface(cx)
                .id("inspector-tree-island")
                .w(tokens::size::SIDE_PANEL_WIDTH)
                .flex_none()
                .child(render_tree(inspector, cx)),
        )
        .child(
            panel_surface(cx)
                .id("inspector-details-island")
                .flex_1()
                .child(details_header)
                .child(
                    scroll_viewport(
                        "inspector-content",
                        ScrollAxis::Both,
                        content_scroll,
                        div()
                            .p_3()
                            .when_some(render_active_summary(inspector, cx), |element, summary| {
                                element.child(summary)
                            })
                            .children(inspector.render_inspector_states(window, cx)),
                        cx,
                    )
                    .min_w(px(0.))
                    .min_h(px(0.))
                    .flex_1(),
                ),
        )
}

fn render_console_tab(
    _inspector: &mut Inspector,
    _window: &mut Window,
    cx: &mut Context<Inspector>,
) -> Div {
    let entity = cx.entity_id();
    let entries = store(cx).entries();
    let scroll = InspectorController::console_scroll(entity, cx);
    let list = InspectorController::console_list(entity, cx);
    let entry_count = entries.len();
    let wheel_state = scroll.clone();
    let entry_list = gpui::uniform_list("console-entry-list", entry_count, move |range, _, cx| {
        range
            .map(|index| render_console_entry(index, entries[index].as_ref(), cx))
            .collect::<Vec<_>>()
    })
    .size_full()
    .track_scroll(list)
    .on_scroll_wheel(move |_, _, _| wheel_state.reveal());

    div()
        .size_full()
        .min_h(px(0.))
        .min_w(px(0.))
        .flex()
        .flex_col()
        .child(
            div()
                .id("console-filter-bar")
                .flex()
                .items_center()
                .px(tokens::spacing::SM)
                .py(px(4.))
                .gap(px(4.))
                .bg(theme::colors(cx).background_secondary)
                .border_b_1()
                .border_color(theme::colors(cx).border_default)
                .child(
                    div()
                        .px(tokens::spacing::SM)
                        .py(px(2.))
                        .rounded(px(4.))
                        .bg(theme::colors(cx).button_background_hover)
                        .text_size(px(11.))
                        .child("Source: All"),
                )
                .child(
                    div()
                        .px(tokens::spacing::SM)
                        .py(px(2.))
                        .rounded(px(4.))
                        .bg(theme::colors(cx).button_background_hover)
                        .text_size(px(11.))
                        .child("Level: All"),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .px(tokens::spacing::SM)
                        .py(px(2.))
                        .rounded(px(4.))
                        .bg(theme::colors(cx).button_background_hover)
                        .text_size(px(11.))
                        .child("Filter..."),
                )
                .child(
                    header_button("console-clear", "Clear", cx)
                        .cursor_pointer()
                        .on_click(cx.listener(|_, _, _, cx| {
                            store(cx).clear();
                            cx.notify();
                        })),
                ),
        )
        .child(
            scroll_surface(ScrollAxis::Vertical, &scroll, entry_list, cx)
                .min_h(px(0.))
                .min_w(px(0.))
                .flex_1(),
        )
}

fn render_console_entry(
    index: usize,
    entry: &DiagnosticLogEntry,
    cx: &gpui::App,
) -> gpui::Stateful<Div> {
    let level_color = match entry.level {
        DiagnosticLevel::Error => theme::colors(cx).text_dangerous,
        DiagnosticLevel::Warn => theme::colors(cx).text_warning,
        DiagnosticLevel::Info => theme::colors(cx).text_primary,
        DiagnosticLevel::Debug => theme::colors(cx).text_secondary,
        DiagnosticLevel::Trace => theme::colors(cx).text_secondary,
    };
    let fields = entry
        .fields
        .as_object()
        .filter(|fields| !fields.is_empty())
        .map(|_| entry.fields.to_string())
        .unwrap_or_default();
    let message = if fields.is_empty() {
        entry.message.clone()
    } else {
        format!("{} {}", entry.message, fields)
    };
    div()
        .id(("console-entry", index))
        .h(px(22.))
        .flex()
        .items_center()
        .px(tokens::spacing::SM)
        .gap(px(6.))
        .text_size(px(12.))
        .font_family("Cascadia Mono")
        .child(
            div()
                .text_color(theme::colors(cx).text_secondary)
                .w(px(36.))
                .child(entry.sequence.to_string()),
        )
        .child(
            div()
                .text_color(level_color)
                .child(format_time_of_day(entry.occurred_at)),
        )
        .child(
            div()
                .text_color(level_color)
                .w(px(45.))
                .child(entry.level.title()),
        )
        .child(
            div()
                .text_color(theme::colors(cx).text_secondary)
                .w(px(60.))
                .child(entry.source.title()),
        )
        .child(
            div()
                .text_color(theme::colors(cx).text_secondary)
                .w(px(120.))
                .truncate()
                .child(entry.target.clone()),
        )
        .child(div().flex_1().min_w(px(0.)).truncate().child(message))
}

fn render_network_tab(cx: &mut Context<Inspector>) -> Div {
    let entries = super::network::store::store(cx).entries();
    let is_recording = super::network::store::store(cx).is_recording();
    let scroll = InspectorController::network_scroll(cx.entity_id(), cx);
    div()
        .size_full()
        .min_h(px(0.))
        .min_w(px(0.))
        .flex()
        .flex_col()
        .child(
            div()
                .id("network-filter-bar")
                .flex()
                .items_center()
                .px(tokens::spacing::SM)
                .py(px(4.))
                .gap(px(4.))
                .bg(theme::colors(cx).background_secondary)
                .border_b_1()
                .border_color(theme::colors(cx).border_default)
                .child(
                    header_button(
                        "network-record",
                        if is_recording {
                            "● Record"
                        } else {
                            "○ Record"
                        },
                        cx,
                    )
                    .cursor_pointer()
                    .when(is_recording, |el| {
                        el.text_color(theme::colors(cx).text_dangerous)
                    })
                    .on_click(cx.listener(|_, _, _, cx| {
                        let store = super::network::store::store(cx);
                        store.set_recording(!store.is_recording());
                        cx.notify();
                    })),
                )
                .child(
                    header_button("network-clear", "Clear", cx)
                        .cursor_pointer()
                        .on_click(cx.listener(|_, _, _, cx| {
                            super::network::store::store(cx).clear();
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .px(tokens::spacing::SM)
                        .py(px(2.))
                        .rounded(px(4.))
                        .bg(theme::colors(cx).button_background_hover)
                        .text_size(px(11.))
                        .child("Filter..."),
                ),
        )
        .child(
            scroll_viewport(
                "network-scroll",
                ScrollAxis::Both,
                &scroll,
                div()
                    .id("network-entry-list")
                    .min_h(px(0.))
                    .flex_1()
                    .children(entries.iter().enumerate().map(|(i, entry)| {
                        let duration_str = entry
                            .duration
                            .map(|d| format!("{:.1} ms", d.as_secs_f64() * 1000.0))
                            .unwrap_or_else(|| "-".to_string());
                        let status_color = match entry.status {
                            super::network::entry::NetworkStatus::Ok => {
                                theme::colors(cx).text_primary
                            }
                            super::network::entry::NetworkStatus::Error => {
                                theme::colors(cx).text_dangerous
                            }
                            super::network::entry::NetworkStatus::Pending => {
                                theme::colors(cx).text_secondary
                            }
                        };
                        let time_str = format_elapsed(entry.occurred_at.elapsed());
                        div()
                            .id(("network-entry", i))
                            .flex()
                            .items_baseline()
                            .px(tokens::spacing::SM)
                            .py(px(2.))
                            .gap(px(6.))
                            .text_size(px(12.))
                            .font_family("Cascadia Mono")
                            .child(
                                div()
                                    .text_color(theme::colors(cx).text_secondary)
                                    .child(time_str),
                            )
                            .child(
                                div()
                                    .text_color(theme::colors(cx).text_secondary)
                                    .w(px(50.))
                                    .child(entry.network_type.title()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .truncate()
                                    .child(entry.method.clone()),
                            )
                            .child(
                                div()
                                    .text_color(status_color)
                                    .w(px(50.))
                                    .child(entry.status.title()),
                            )
                            .child(
                                div()
                                    .text_color(theme::colors(cx).text_secondary)
                                    .w(px(70.))
                                    .child(duration_str),
                            )
                    })),
                cx,
            )
            .min_h(px(0.))
            .min_w(px(0.))
            .flex_1(),
        )
}

fn render_performance_tab(cx: &mut Context<Inspector>) -> Div {
    let entries = super::performance::store::store(cx).entries();
    let is_recording = super::performance::store::store(cx).is_recording();
    let scroll = InspectorController::performance_scroll(cx.entity_id(), cx);
    div()
        .size_full()
        .min_h(px(0.))
        .min_w(px(0.))
        .flex()
        .flex_col()
        .child(
            div()
                .id("performance-filter-bar")
                .flex()
                .items_center()
                .px(tokens::spacing::SM)
                .py(px(4.))
                .gap(px(4.))
                .bg(theme::colors(cx).background_secondary)
                .border_b_1()
                .border_color(theme::colors(cx).border_default)
                .child(
                    header_button(
                        "performance-record",
                        if is_recording {
                            "● Record"
                        } else {
                            "○ Record"
                        },
                        cx,
                    )
                    .cursor_pointer()
                    .when(is_recording, |el| {
                        el.text_color(theme::colors(cx).text_dangerous)
                    })
                    .on_click(cx.listener(|_, _, _, cx| {
                        let store = super::performance::store::store(cx);
                        store.set_recording(!store.is_recording());
                        cx.notify();
                    })),
                )
                .child(
                    header_button("performance-clear", "Clear", cx)
                        .cursor_pointer()
                        .on_click(cx.listener(|_, _, _, cx| {
                            super::performance::store::store(cx).clear();
                            cx.notify();
                        })),
                ),
        )
        .child(
            scroll_viewport(
                "performance-scroll",
                ScrollAxis::Both,
                &scroll,
                div()
                    .id("performance-entry-list")
                    .min_h(px(0.))
                    .flex_1()
                    .children(entries.iter().enumerate().map(|(i, entry)| {
                        let duration_ms = entry.duration.as_secs_f64() * 1000.0;
                        let slow = duration_ms > super::performance::store::SLOW_FRAME_THRESHOLD_MS;
                        let status_color = if slow {
                            theme::colors(cx).text_dangerous
                        } else {
                            theme::colors(cx).text_primary
                        };
                        div()
                            .id(("performance-entry", i))
                            .flex()
                            .items_baseline()
                            .px(tokens::spacing::SM)
                            .py(px(2.))
                            .gap(px(6.))
                            .text_size(px(12.))
                            .font_family("Cascadia Mono")
                            .child(
                                div()
                                    .text_color(theme::colors(cx).text_secondary)
                                    .w(px(80.))
                                    .child(entry.label.clone()),
                            )
                            .child(
                                div()
                                    .text_color(status_color)
                                    .w(px(80.))
                                    .child(format!("{:.1} ms", duration_ms)),
                            )
                            .child(
                                div().text_color(theme::colors(cx).text_secondary).child(
                                    entry
                                        .frame_number
                                        .map(|f| format!("Frame #{}", f))
                                        .unwrap_or_default(),
                                ),
                            )
                            .when(slow, |el| el.child(div().child("⚠ Slow")))
                    })),
                cx,
            )
            .min_h(px(0.))
            .min_w(px(0.))
            .flex_1(),
        )
}

fn render_state_tab(
    _inspector: &mut gpui::Inspector,
    _window: &mut Window,
    cx: &mut Context<gpui::Inspector>,
) -> Div {
    let store = super::state::store::store(cx);
    let snapshot = store.current();
    let is_watch = store.is_watch_enabled();
    let watch_history = store.watch_history();
    let entity_id = cx.entity_id();
    let tree_scroll = InspectorController::state_tree_scroll(entity_id, cx);
    let watch_scroll = InspectorController::state_watch_scroll(entity_id, cx);

    // ツリーノード一覧。子ノードと各エントリを再帰的に表示する。
    let tree_nodes: Vec<_> = snapshot
        .as_ref()
        .map(|snap| {
            snap.nodes
                .iter()
                .map(|node| render_state_tree_node(node, 0, cx))
                .collect()
        })
        .unwrap_or_default();

    let has_watch_history = !watch_history.is_empty();
    // Watch 履歴エントリ
    let watch_entries: Vec<_> = watch_history
        .iter()
        .rev()
        .take(50)
        .enumerate()
        .map(|(i, event)| {
            let time_str = format_elapsed(event.occurred_at.elapsed().unwrap_or_default());
            let path = event.path.clone();
            let old_value = event.old_value.clone();
            let new_value = event.new_value.clone();
            div()
                .id(("watch-event", i))
                .flex()
                .items_baseline()
                .gap(px(4.))
                .text_size(px(11.))
                .font_family("Cascadia Mono")
                .child(
                    div()
                        .text_color(theme::colors(cx).text_secondary)
                        .child(time_str),
                )
                .child(div().text_color(theme::colors(cx).text_primary).child(path))
                .child(
                    div()
                        .text_color(theme::colors(cx).text_secondary)
                        .child(old_value),
                )
                .child(
                    div()
                        .text_color(theme::colors(cx).text_secondary)
                        .child("→"),
                )
                .child(
                    div()
                        .text_color(theme::colors(cx).text_primary)
                        .child(new_value),
                )
        })
        .collect();

    // Snapshot の取得時刻を表示する。描画時刻ではない。
    let updated_str = snapshot
        .as_ref()
        .map(|snap| format!("Updated: {}", format_time_of_day(snap.captured_at)))
        .unwrap_or_else(|| "No snapshot".to_string());

    div()
        .size_full()
        .min_h(px(0.))
        .min_w(px(0.))
        .flex()
        .flex_col()
        // ヘッダー
        .child(
            div()
                .id("state-header")
                .flex()
                .items_center()
                .px(tokens::spacing::SM)
                .py(px(4.))
                .gap(px(4.))
                .bg(theme::colors(cx).background_secondary)
                .border_b_1()
                .border_color(theme::colors(cx).border_default)
                .child(
                    header_button("state-snapshot", "Snapshot", cx)
                        .cursor_pointer()
                        .on_click(cx.listener(move |_, _, _, cx| {
                            let target = InspectorController::target(entity_id, cx);
                            if let Some(target) = target {
                                let _ = target.update(cx, |_, window, cx| {
                                    let snap = super::state::snapshot::capture_state(window, cx);
                                    super::state::store::store(cx).set_snapshot(snap);
                                });
                            }
                            cx.notify();
                        })),
                )
                .child(
                    header_button(
                        "state-watch",
                        if is_watch { "Watch: On" } else { "Watch: Off" },
                        cx,
                    )
                    .cursor_pointer()
                    .when(is_watch, |el| {
                        el.text_color(theme::colors(cx).text_dangerous)
                    })
                    .on_click(cx.listener(move |_, _, _, cx| {
                        super::state::store::store(cx)
                            .set_watch_enabled(!super::state::store::store(cx).is_watch_enabled());
                        cx.notify();
                    })),
                )
                .child(
                    header_button("state-clear", "Clear", cx)
                        .cursor_pointer()
                        .on_click(cx.listener(|_, _, _, cx| {
                            super::state::store::store(cx).clear();
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .ml_auto()
                        .text_size(px(11.))
                        .text_color(theme::colors(cx).text_secondary)
                        .child(updated_str),
                ),
        )
        // 2ペイン: ツリー | Watch 履歴
        .child(
            div()
                .id("state-content")
                .flex_1()
                .min_h(px(0.))
                .min_w(px(0.))
                .flex()
                // 左: ツリー
                .child(
                    div()
                        .id("state-tree")
                        .w(px(360.))
                        .min_w(px(220.))
                        .flex()
                        .flex_col()
                        .border_r_1()
                        .border_color(theme::colors(cx).border_default)
                        .child(
                            scroll_viewport(
                                "state-tree-scroll",
                                ScrollAxis::Both,
                                &tree_scroll,
                                div()
                                    .id("state-tree-list")
                                    .min_h(px(0.))
                                    .flex_1()
                                    .px(tokens::spacing::SM)
                                    .py(px(4.))
                                    .gap(px(1.))
                                    .children(tree_nodes)
                                    .when(snapshot.is_none(), |el| {
                                        el.child(
                                            div()
                                                .py_4()
                                                .text_size(px(12.))
                                                .text_color(theme::colors(cx).text_secondary)
                                                .child("Snapshot をクリックして状態を取得"),
                                        )
                                    }),
                                cx,
                            )
                            .min_h(px(0.))
                            .flex_1(),
                        ),
                )
                // 右: Watch 履歴
                .child(
                    div()
                        .id("state-details")
                        .flex_1()
                        .min_w(px(0.))
                        .min_h(px(0.))
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .id("state-watch-history")
                                .flex_1()
                                .min_h(px(0.))
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .px(tokens::spacing::SM)
                                        .py(px(2.))
                                        .text_size(px(11.))
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_color(theme::colors(cx).text_secondary)
                                        .child("Watch History"),
                                )
                                .when(has_watch_history, |el| {
                                    el.child(
                                        scroll_viewport(
                                            "state-watch-scroll",
                                            ScrollAxis::Vertical,
                                            &watch_scroll,
                                            div()
                                                .id("state-watch-list")
                                                .min_h(px(0.))
                                                .flex_1()
                                                .px(tokens::spacing::SM)
                                                .py(px(2.))
                                                .gap(px(1.))
                                                .children(watch_entries),
                                            cx,
                                        )
                                        .min_h(px(0.))
                                        .flex_1(),
                                    )
                                })
                                .when(!has_watch_history, |el| {
                                    el.child(
                                        div()
                                            .px(tokens::spacing::SM)
                                            .py(px(4.))
                                            .text_size(px(12.))
                                            .text_color(theme::colors(cx).text_secondary)
                                            .child(
                                                "Watch を On にして Snapshot を取得すると、変更履歴がここに表示されます",
                                            ),
                                    )
                                }),
                        ),
                ),
        )
}

fn render_state_tree_node(node: &StateNode, depth: usize, cx: &gpui::App) -> Div {
    let indent = depth * 14;
    let mut container = div().flex().flex_col().gap(px(1.)).child(
        div()
            .flex()
            .items_center()
            .py(px(1.))
            .pl(px(indent as f32))
            .text_size(px(12.))
            .font_family("Cascadia Mono")
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(theme::colors(cx).text_primary)
            .child(node.name.clone()),
    );
    for entry in &node.entries {
        container = container.child(render_state_entry(entry, depth + 1, cx));
    }
    for child in &node.children {
        container = container.child(render_state_tree_node(child, depth + 1, cx));
    }
    container
}

fn render_state_entry(entry: &StateEntry, depth: usize, cx: &gpui::App) -> Div {
    div()
        .flex()
        .items_baseline()
        .gap(px(4.))
        .pl(px((depth * 14) as f32))
        .text_size(px(11.))
        .font_family("Cascadia Mono")
        .child(
            div()
                .text_color(theme::colors(cx).text_secondary)
                .child(format!("{}:", entry.key)),
        )
        .child(
            div()
                .text_color(theme::colors(cx).text_primary)
                .child(entry.value.clone()),
        )
}

/// Duration を MM:SS.mmm 形式で表示する。
fn format_elapsed(duration: Duration) -> String {
    format!(
        "{:02}:{:02}.{:03}",
        (duration.as_secs() % 3_600) / 60,
        duration.as_secs() % 60,
        duration.subsec_millis()
    )
}

/// SystemTime を UTC の HH:MM:SS 形式で表示する。
fn format_time_of_day(time: SystemTime) -> String {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| {
            let secs = duration.as_secs() % 86_400;
            format!(
                "{:02}:{:02}:{:02}",
                secs / 3_600,
                (secs % 3_600) / 60,
                secs % 60
            )
        })
        .unwrap_or_else(|_| "unknown".to_string())
}
