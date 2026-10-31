//! Console UI の表示と状態管理。
//!
//! フィルター（Source/Level/Text）、Clear、Auto scroll、Details 表示を提供する。

use gpui::{
    Context, Div, EventEmitter, IntoElement, ParentElement, Render, Styled, Subscription, Window,
    div, prelude::*, px,
};

use crate::{
    components::{ScrollAxis, ScrollState, header_button, scroll_viewport},
    theme, tokens,
};

use super::entry::{DiagnosticLevel, DiagnosticLogEntry, DiagnosticSource};
use super::store::store;

/// Console のフィルター状態。
#[derive(Clone, Debug, PartialEq, Eq)]
struct ConsoleFilter {
    source: Option<DiagnosticSource>,
    level: Option<DiagnosticLevel>,
    text: String,
}

impl ConsoleFilter {
    fn matches(&self, entry: &DiagnosticLogEntry) -> bool {
        if self.source.is_some_and(|s| entry.source != s) {
            return false;
        }
        if self.level.is_some_and(|l| entry.level != l) {
            return false;
        }
        if !self.text.is_empty() {
            let text = self.text.to_lowercase();
            if !entry.message.to_lowercase().contains(&text)
                && !entry.target.to_lowercase().contains(&text)
            {
                return false;
            }
        }
        true
    }
}

/// Console View の状態。
pub(crate) struct ConsoleView {
    scroll: ScrollState,
    filter: ConsoleFilter,
    selected_index: Option<usize>,
    /// フィルター後のエントリ一覧。
    filtered_entries: Vec<DiagnosticLogEntry>,
    /// 選択中のエントリ。
    selected_entry: Option<DiagnosticLogEntry>,
    /// store の変更を監視するためのサブスクリプション。
    _refresh_subscription: Option<Subscription>,
}

impl EventEmitter<()> for ConsoleView {}

impl ConsoleView {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let entries = store(cx).entries();
        Self {
            scroll: ScrollState::default(),
            filter: ConsoleFilter {
                source: None,
                level: None,
                text: String::new(),
            },
            selected_index: None,
            filtered_entries: entries,
            selected_entry: None,
            _refresh_subscription: None,
        }
    }

    /// エントリを再読み込みしてフィルターを適用する。
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let all = store(cx).entries();
        self.filtered_entries
            .retain(|entry| self.filter.matches(entry));
        // 新しいエントリを追加
        for entry in &all {
            if !self
                .filtered_entries
                .iter()
                .any(|e| e.sequence == entry.sequence)
                && self.filter.matches(entry)
            {
                self.filtered_entries.push(entry.clone());
            }
        }
        // シーケンス順にソート
        self.filtered_entries.sort_by_key(|e| e.sequence);

        // 選択中のインデックスを更新
        if let Some(ref selected) = self.selected_entry {
            self.selected_index = self
                .filtered_entries
                .iter()
                .position(|e| e.sequence == selected.sequence);
        }

        cx.notify();
    }

    /// store をクリアする。
    fn clear(&mut self, cx: &mut Context<Self>) {
        store(cx).clear();
        self.filtered_entries.clear();
        self.selected_entry = None;
        self.selected_index = None;
        cx.notify();
    }

    /// エントリを選択する。
    fn select_entry(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.filtered_entries.len() {
            self.selected_index = Some(index);
            self.selected_entry = Some(self.filtered_entries[index].clone());
            cx.notify();
        }
    }

    /// Source フィルターを設定する。
    fn set_source_filter(&mut self, source: Option<DiagnosticSource>, cx: &mut Context<Self>) {
        self.filter.source = source;
        self.refresh(cx);
    }

    /// Level フィルターを設定する。
    fn set_level_filter(&mut self, level: Option<DiagnosticLevel>, cx: &mut Context<Self>) {
        self.filter.level = level;
        self.refresh(cx);
    }
}

impl Render for ConsoleView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let entries = &self.filtered_entries;
        let selected = &self.selected_entry;

        // フィルターバー
        let filter_bar = div()
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
                // Source フィルター
                div()
                    .id("console-source-filter")
                    .px(tokens::spacing::SM)
                    .py(px(2.))
                    .rounded(px(4.))
                    .bg(theme::colors().button_background_hover)
                    .text_size(px(11.))
                    .cursor_pointer()
                    .child(if self.filter.source.is_none() {
                        "Source: All"
                    } else {
                        "Source: Desktop"
                    })
                    .on_click(cx.listener(|view, _, _, cx| {
                        // Source が All なら Desktop に、Desktop なら All に切り替え
                        let next = if view.filter.source.is_none() {
                            Some(DiagnosticSource::Desktop)
                        } else {
                            None
                        };
                        view.set_source_filter(next, cx);
                    })),
            )
            .child(
                // Level フィルター
                div()
                    .id("console-level-filter")
                    .px(tokens::spacing::SM)
                    .py(px(2.))
                    .rounded(px(4.))
                    .bg(theme::colors().button_background_hover)
                    .text_size(px(11.))
                    .cursor_pointer()
                    .child(self.filter.level.map(|l| l.title()).unwrap_or("Level: All"))
                    .on_click(cx.listener(|view, _, _, cx| {
                        // Level を循環切り替え: None -> Error -> Warn -> Info -> Debug -> Trace -> None
                        let next = match view.filter.level {
                            None => Some(DiagnosticLevel::Error),
                            Some(DiagnosticLevel::Error) => Some(DiagnosticLevel::Warn),
                            Some(DiagnosticLevel::Warn) => Some(DiagnosticLevel::Info),
                            Some(DiagnosticLevel::Info) => Some(DiagnosticLevel::Debug),
                            Some(DiagnosticLevel::Debug) => Some(DiagnosticLevel::Trace),
                            Some(DiagnosticLevel::Trace) => None,
                        };
                        view.set_level_filter(next, cx);
                    })),
            )
            .child(
                // テキストフィルター
                div()
                    .id("console-text-filter")
                    .flex_1()
                    .min_w(px(0.))
                    .px(tokens::spacing::SM)
                    .py(px(2.))
                    .rounded(px(4.))
                    .bg(theme::colors().button_background_hover)
                    .text_size(px(11.))
                    .child(format!("Filter...{}", self.filter.text)),
            )
            .child(
                // Clear ボタン
                header_button("console-clear", "Clear")
                    .cursor_pointer()
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.clear(cx);
                    })),
            );

        // エントリ一覧と詳細ペイン
        let content = div()
            .id("console-content")
            .size_full()
            .flex()
            .child(
                // エントリ一覧
                div()
                    .id("console-entries")
                    .flex_1()
                    .min_w(px(0.))
                    .border_r_1()
                    .border_color(theme::colors().border_default)
                    .child(
                        scroll_viewport(
                            "console-scroll",
                            ScrollAxis::Both,
                            &self.scroll,
                            div()
                                .id("console-entry-list")
                                .min_h(px(0.))
                                .flex_1()
                                .children(entries.iter().enumerate().map(|(i, entry)| {
                                    let is_selected =
                                        self.selected_index.is_some_and(|idx| idx == i);
                                    let level_color = match entry.level {
                                        DiagnosticLevel::Error => theme::colors().text_dangerous,
                                        DiagnosticLevel::Warn => theme::colors().text_warning,
                                        DiagnosticLevel::Info => theme::colors().text_primary,
                                        DiagnosticLevel::Debug => theme::colors().text_secondary,
                                        DiagnosticLevel::Trace => theme::colors().text_secondary,
                                    };
                                    // タイムスタンプをフォーマット
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
                                        .cursor_pointer()
                                        .text_size(px(12.))
                                        .font_family("Cascadia Mono")
                                        .when(is_selected, |el| {
                                            el.bg(theme::colors().button_background_selected)
                                        })
                                        .on_click(cx.listener(move |view, _, _, cx| {
                                            view.select_entry(i, cx);
                                        }))
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
                    ),
            )
            .when_some(selected.clone(), |el, entry| {
                el.child(
                    // 詳細ペイン
                    div()
                        .id("console-details")
                        .w(tokens::size::SIDE_PANEL_WIDTH)
                        .flex_none()
                        .p(tokens::spacing::SM)
                        .gap(px(4.))
                        .flex_col()
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child("Details"),
                        )
                        .child(render_detail_row("Sequence", entry.sequence.to_string()))
                        .child({
                            let timestamp = entry
                                .occurred_at
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| {
                                    let secs = d.as_secs();
                                    format!("{} seconds since epoch", secs)
                                })
                                .unwrap_or_else(|_| "unknown".to_string());
                            render_detail_row("Timestamp", &timestamp)
                        })
                        .child(render_detail_row("Level", entry.level.title()))
                        .child(render_detail_row("Source", entry.source.title()))
                        .child(render_detail_row("Target", &entry.target))
                        .child(render_detail_row("Message", &entry.message))
                        .child(
                            div()
                                .text_size(px(11.))
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .mt_2()
                                .child("Fields"),
                        )
                        .child(
                            div().text_size(px(11.)).font_family("Cascadia Mono").child(
                                entry
                                    .fields
                                    .as_object()
                                    .map(|obj| {
                                        if obj.is_empty() {
                                            "(none)".to_string()
                                        } else {
                                            serde_json::to_string_pretty(obj)
                                                .unwrap_or_else(|_| "(error)".to_string())
                                        }
                                    })
                                    .unwrap_or_else(|| "(none)".to_string()),
                            ),
                        ),
                )
            });

        div()
            .size_full()
            .flex()
            .flex_col()
            .child(filter_bar)
            .child(content)
            .into_any_element()
    }
}

fn render_detail_row(label: impl Into<String>, value: impl Into<String>) -> Div {
    let label = label.into();
    let value = value.into();
    div()
        .flex()
        .gap(px(4.))
        .text_size(px(11.))
        .child(
            div()
                .w(px(70.))
                .flex_none()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme::colors().text_secondary)
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .font_family("Cascadia Mono")
                .child(value),
        )
}
