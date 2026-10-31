use gpui::{AnyElement, Context, Div, Inspector, IntoElement, Window, div, prelude::*, px};

use crate::{
    components::{
        ScrollAxis, ScrollState, header_button, panel_header, panel_surface, scroll_viewport,
    },
    theme, tokens,
};

use super::console::store::store;
use super::console::view::ConsoleView;
use super::elements::tree_view::{render_active_summary, render_tree};
use super::inspector_controller::InspectorController;
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
        .bg(theme::colors().background_secondary)
        .border_b_1()
        .border_color(theme::colors().border_default)
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
                    el.bg(theme::colors().button_background_selected)
                        .text_color(theme::colors().text_primary)
                })
                .when(!is_active, |el| {
                    el.text_color(theme::colors().text_secondary)
                })
                .when(!is_active, |el| {
                    el.bg(theme::colors().button_background_hover)
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
        .bg(theme::colors().background_primary)
        .text_color(theme::colors().text_primary)
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
            header_button("inspector-refresh", "Refresh")
                .cursor_pointer()
                .on_click(cx.listener(|_, _, _, cx| {
                    InspectorController::defer(cx, InspectorController::refresh_target);
                })),
        )
        .child(
            header_button("inspector-pick", "Pick element")
                .cursor_pointer()
                .when(inspector.is_picking(), |button| {
                    button.bg(theme::colors().button_background_selected)
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
            panel_surface()
                .id("inspector-tree-island")
                .w(tokens::size::SIDE_PANEL_WIDTH)
                .flex_none()
                .child(render_tree(inspector, cx)),
        )
        .child(
            panel_surface()
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
                            .when_some(render_active_summary(inspector), |element, summary| {
                                element.child(summary)
                            })
                            .children(inspector.render_inspector_states(window, cx)),
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
    let _ = cx.new(ConsoleView::new);
    // ConsoleView は Entity として保持され、render_inspector の再描画時に更新される
    let entries = store(cx).entries();
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
                .bg(theme::colors().background_secondary)
                .border_b_1()
                .border_color(theme::colors().border_default)
                .child(
                    div()
                        .px(tokens::spacing::SM)
                        .py(px(2.))
                        .rounded(px(4.))
                        .bg(theme::colors().button_background_hover)
                        .text_size(px(11.))
                        .child("Source: All"),
                )
                .child(
                    div()
                        .px(tokens::spacing::SM)
                        .py(px(2.))
                        .rounded(px(4.))
                        .bg(theme::colors().button_background_hover)
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
                        .bg(theme::colors().button_background_hover)
                        .text_size(px(11.))
                        .child("Filter..."),
                )
                .child(
                    header_button("console-clear", "Clear")
                        .cursor_pointer()
                        .on_click(cx.listener(|_, _, _, cx| {
                            store(cx).clear();
                            cx.notify();
                        })),
                ),
        )
        .child(
            scroll_viewport(
                "console-scroll",
                ScrollAxis::Both,
                &ScrollState::default(),
                div()
                    .id("console-entry-list")
                    .min_h(px(0.))
                    .flex_1()
                    .children(entries.iter().enumerate().map(|(i, entry)| {
                        let level_color = match entry.level {
                            super::console::entry::DiagnosticLevel::Error => {
                                theme::colors().text_dangerous
                            }
                            super::console::entry::DiagnosticLevel::Warn => {
                                theme::colors().text_warning
                            }
                            super::console::entry::DiagnosticLevel::Info => {
                                theme::colors().text_primary
                            }
                            super::console::entry::DiagnosticLevel::Debug => {
                                theme::colors().text_secondary
                            }
                            super::console::entry::DiagnosticLevel::Trace => {
                                theme::colors().text_secondary
                            }
                        };
                        let timestamp = entry
                            .occurred_at
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| {
                                let secs = d.as_secs() % 86400;
                                let h = secs / 3600;
                                let m = (secs % 3600) / 60;
                                let s = secs % 60;
                                format!("{:02}:{:02}:{:02}", h, m, s)
                            })
                            .unwrap_or_else(|_| "00:00:00".to_string());
                        div()
                            .id(("console-entry", i))
                            .flex()
                            .items_baseline()
                            .px(tokens::spacing::SM)
                            .py(px(2.))
                            .gap(px(6.))
                            .text_size(px(12.))
                            .font_family("Cascadia Mono")
                            .child(div().text_color(level_color).child(timestamp))
                            .child(
                                div()
                                    .text_color(level_color)
                                    .w(px(45.))
                                    .child(entry.level.title()),
                            )
                            .child(
                                div()
                                    .text_color(theme::colors().text_secondary)
                                    .w(px(60.))
                                    .child(entry.source.title()),
                            )
                            .child(
                                div()
                                    .text_color(theme::colors().text_secondary)
                                    .w(px(120.))
                                    .truncate()
                                    .child(entry.target.clone()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .truncate()
                                    .child(entry.message.clone()),
                            )
                    })),
            )
            .min_h(px(0.))
            .min_w(px(0.))
            .flex_1(),
        )
}

fn render_network_tab(cx: &mut Context<Inspector>) -> Div {
    let entries = super::network::store::store(cx).entries();
    let is_recording = super::network::store::store(cx).is_recording();
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
                .bg(theme::colors().background_secondary)
                .border_b_1()
                .border_color(theme::colors().border_default)
                .child(
                    header_button(
                        "network-record",
                        if is_recording {
                            "● Record"
                        } else {
                            "○ Record"
                        },
                    )
                    .cursor_pointer()
                    .when(is_recording, |el| {
                        el.text_color(theme::colors().text_dangerous)
                    })
                    .on_click(cx.listener(|_, _, _, cx| {
                        let store = super::network::store::store(cx);
                        store.set_recording(!store.is_recording());
                        cx.notify();
                    })),
                )
                .child(
                    header_button("network-clear", "Clear")
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
                        .bg(theme::colors().button_background_hover)
                        .text_size(px(11.))
                        .child("Filter..."),
                ),
        )
        .child(
            scroll_viewport(
                "network-scroll",
                ScrollAxis::Both,
                &ScrollState::default(),
                div()
                    .id("network-entry-list")
                    .min_h(px(0.))
                    .flex_1()
                    .children(entries.iter().enumerate().map(|(i, entry)| {
                        let duration_str = entry
                            .duration
                            .map(|d| format!("{:.1} ms", d.as_millis() as f64))
                            .unwrap_or_else(|| "-".to_string());
                        let status_color = match entry.status {
                            super::network::entry::NetworkStatus::Ok => {
                                theme::colors().text_primary
                            }
                            super::network::entry::NetworkStatus::Error => {
                                theme::colors().text_dangerous
                            }
                            super::network::entry::NetworkStatus::Pending => {
                                theme::colors().text_secondary
                            }
                        };
                        let timestamp = entry.occurred_at.elapsed();
                        let time_str = format!(
                            "{:02}:{:02}.{:03}",
                            timestamp.as_secs() % 3600 % 60,
                            timestamp.as_secs() % 60,
                            timestamp.subsec_millis()
                        );
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
                                    .text_color(theme::colors().text_secondary)
                                    .child(time_str),
                            )
                            .child(
                                div()
                                    .text_color(theme::colors().text_secondary)
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
                                    .text_color(theme::colors().text_secondary)
                                    .w(px(70.))
                                    .child(duration_str),
                            )
                    })),
            )
            .min_h(px(0.))
            .min_w(px(0.))
            .flex_1(),
        )
}

fn render_performance_tab(cx: &mut Context<Inspector>) -> Div {
    let entries = super::performance::store::store(cx).entries();
    let is_recording = super::performance::store::store(cx).is_recording();
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
                .bg(theme::colors().background_secondary)
                .border_b_1()
                .border_color(theme::colors().border_default)
                .child(
                    header_button(
                        "performance-record",
                        if is_recording {
                            "● Record"
                        } else {
                            "○ Record"
                        },
                    )
                    .cursor_pointer()
                    .when(is_recording, |el| {
                        el.text_color(theme::colors().text_dangerous)
                    })
                    .on_click(cx.listener(|_, _, _, cx| {
                        let store = super::performance::store::store(cx);
                        store.set_recording(!store.is_recording());
                        cx.notify();
                    })),
                )
                .child(
                    header_button("performance-clear", "Clear")
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
                &ScrollState::default(),
                div()
                    .id("performance-entry-list")
                    .min_h(px(0.))
                    .flex_1()
                    .children(entries.iter().enumerate().map(|(i, entry)| {
                        let duration_ms = entry.duration.as_millis() as f64;
                        let slow = duration_ms > 16.67;
                        let status_color = if slow {
                            theme::colors().text_dangerous
                        } else {
                            theme::colors().text_primary
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
                                    .text_color(theme::colors().text_secondary)
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
                                div().text_color(theme::colors().text_secondary).child(
                                    entry
                                        .frame_number
                                        .map(|f| format!("Frame #{}", f))
                                        .unwrap_or_default(),
                                ),
                            )
                            .when(slow, |el| el.child(div().child("⚠ Slow")))
                    })),
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

    // ツリーノード一覧
    let tree_nodes: Vec<_> = snapshot
        .as_ref()
        .map(|snap| {
            snap.nodes
                .iter()
                .enumerate()
                .map(|(i, node)| render_state_tree_node(node, i, 0, cx))
                .collect()
        })
        .unwrap_or_default();

    // Watch 履歴エントリ
    let watch_entries: Vec<_> = watch_history
        .iter()
        .rev()
        .take(50)
        .enumerate()
        .map(|(i, event)| {
            let elapsed = event.occurred_at.elapsed().unwrap_or_default();
            let time_str = format!(
                "{:02}:{:02}.{:03}",
                elapsed.as_secs() % 3600 % 60,
                elapsed.as_secs() % 60,
                elapsed.subsec_millis()
            );
            let path = event.path.clone();
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
                        .text_color(theme::colors().text_secondary)
                        .child(time_str),
                )
                .child(div().text_color(theme::colors().text_primary).child(path))
                .child(div().text_color(theme::colors().text_secondary).child("→"))
                .child(
                    div()
                        .text_color(theme::colors().text_primary)
                        .child(new_value),
                )
        })
        .collect();

    // 更新時刻
    let updated_str = if snapshot.is_some() {
        let now = std::time::SystemTime::now();
        let elapsed = now
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        format!(
            "Updated: {:02}:{:02}.{:03}",
            (elapsed.as_secs() / 3600) % 24,
            (elapsed.as_secs() / 60) % 60,
            elapsed.subsec_millis()
        )
    } else {
        "No snapshot".to_string()
    };

    let entity_id = cx.entity_id();

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
                .bg(theme::colors().background_secondary)
                .border_b_1()
                .border_color(theme::colors().border_default)
                .child(
                    header_button("state-snapshot", "Snapshot")
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
                    )
                    .cursor_pointer()
                    .when(is_watch, |el| el.text_color(theme::colors().text_dangerous))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        super::state::store::store(cx)
                            .set_watch_enabled(!super::state::store::store(cx).is_watch_enabled());
                        cx.notify();
                    })),
                )
                .child(
                    header_button("state-clear", "Clear")
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
                        .text_color(theme::colors().text_secondary)
                        .child(updated_str),
                ),
        )
        // 2ペイン: ツリー | 詳細
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
                        .w(px(280.))
                        .min_w(px(200.))
                        .flex()
                        .flex_col()
                        .border_r_1()
                        .border_color(theme::colors().border_default)
                        .child(scroll_viewport(
                            "state-tree-scroll",
                            ScrollAxis::Both,
                            &ScrollState::default(),
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
                                            .text_color(theme::colors().text_secondary)
                                            .child("Snapshot をクリックして状態を取得"),
                                    )
                                }),
                        )),
                )
                // 右: 詳細 / Watch 履歴
                .child(
                    div()
                        .id("state-details")
                        .flex_1()
                        .min_w(px(0.))
                        .min_h(px(0.))
                        .flex()
                        .flex_col()
                        .when(!watch_entries.is_empty(), |el| {
                            el.child(
                                div()
                                    .id("state-watch-history")
                                    .max_h(px(200.))
                                    .border_b_1()
                                    .border_color(theme::colors().border_default)
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .px(tokens::spacing::SM)
                                            .py(px(2.))
                                            .text_size(px(11.))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(theme::colors().text_secondary)
                                            .child("Watch History"),
                                    )
                                    .child(scroll_viewport(
                                        "state-watch-scroll",
                                        ScrollAxis::Vertical,
                                        &ScrollState::default(),
                                        div()
                                            .id("state-watch-list")
                                            .min_h(px(0.))
                                            .flex_1()
                                            .px(tokens::spacing::SM)
                                            .py(px(2.))
                                            .gap(px(1.))
                                            .children(watch_entries),
                                    )),
                            )
                        })
                        .child(
                            div()
                                .id("state-details-content")
                                .flex_1()
                                .min_h(px(0.))
                                .min_w(px(0.))
                                .px(tokens::spacing::SM)
                                .py(px(4.))
                                .text_size(px(12.))
                                .text_color(theme::colors().text_secondary)
                                .child("ノードを選択して詳細を表示"),
                        ),
                ),
        )
}

fn render_state_tree_node(
    node: &super::state::snapshot::StateNode,
    _index: usize,
    depth: usize,
    _cx: &mut Context<gpui::Inspector>,
) -> Div {
    let has_children = !node.children.is_empty();
    let indent = depth * 16;
    let name = node.name.clone();

    div()
        .flex()
        .items_center()
        .gap(px(2.))
        .py(px(1.))
        .px(px(2.))
        .pl(px(indent as f32))
        .rounded(px(3.))
        .cursor_pointer()
        .text_size(px(12.))
        .font_family("Cascadia Mono")
        .child(
            div()
                .w(px(14.))
                .h(px(14.))
                .flex()
                .items_center()
                .justify_center()
                .when(has_children, |el| el.child("▸")),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(name),
        )
}
