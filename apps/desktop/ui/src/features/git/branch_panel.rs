//! IntelliJ風のブランチ選択パネル
//!
//! 右パネルに配置され、ローカルブランチを階層構造で表示します。
//! ブランチ名のスラッシュ（`/`）に基づいてグループ化し、折りたたみ可能なツリーとして表示します。

use super::GitFeature;
use crate::{
    components::{Icon, IconName, SearchInput, SearchInputEvent, panel_empty_state},
    theme, tokens,
};
use gpui::{
    Context, Entity, FocusHandle, Focusable, Render, Subscription, Window, div, prelude::*, px,
};
use lapis_editor_core::WorkspaceId;
use lapis_git::GitAction;
use std::collections::{BTreeMap, HashSet};

/// git branch -v の出力をパースしたブランチ情報
#[derive(Clone, Debug, PartialEq, Eq)]
struct Branch {
    name: String,
    oid: String,
}

fn parse_branches(output: &str) -> Result<Vec<Branch>, String> {
    output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            let fields = if fields.first() == Some(&"*") {
                &fields[1..]
            } else {
                &fields[..]
            };
            if fields.len() != 2 || !fields[1].bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err("ブランチ一覧の形式を読み取れませんでした".into());
            }
            Ok(Branch {
                name: fields[0].into(),
                oid: fields[1].into(),
            })
        })
        .collect()
}

/// 階層構造のブランチツリーノード
#[derive(Default)]
struct BranchTreeNode {
    children: BTreeMap<String, BranchTreeNode>,
    branch: Option<Branch>,
}

/// ツリー行の種類（グループまたはブランチ）
enum BranchRow {
    Group {
        path: String,
        label: String,
        depth: usize,
        expanded: bool,
    },
    Branch {
        branch: Branch,
        label: String,
        depth: usize,
    },
}

/// ブランチリストを階層構造の行に変換
fn build_branch_rows(
    branches: &[Branch],
    query: &str,
    collapsed: &HashSet<String>,
) -> Vec<BranchRow> {
    let query_lower = query.to_lowercase();
    let mut root = BranchTreeNode::default();

    // 検索クエリに一致するブランチをツリーに配置
    for branch in branches
        .iter()
        .filter(|b| b.name.to_lowercase().contains(&query_lower))
    {
        let mut node = &mut root;
        for segment in branch.name.split('/') {
            node = node.children.entry(segment.into()).or_default();
        }
        node.branch = Some(branch.clone());
    }

    fn append_rows(
        node: BranchTreeNode,
        parent: &str,
        depth: usize,
        has_query: bool,
        collapsed: &HashSet<String>,
        rows: &mut Vec<BranchRow>,
    ) {
        for (label, child) in node.children {
            let path = if parent.is_empty() {
                label.clone()
            } else {
                format!("{parent}/{label}")
            };

            if let Some(branch) = child.branch {
                rows.push(BranchRow::Branch {
                    branch,
                    label,
                    depth,
                });
            } else {
                let expanded = has_query || !collapsed.contains(&path);
                rows.push(BranchRow::Group {
                    path: path.clone(),
                    label,
                    depth,
                    expanded,
                });
                if expanded {
                    append_rows(child, &path, depth + 1, has_query, collapsed, rows);
                }
            }
        }
    }

    let mut rows = Vec::new();
    append_rows(root, "", 0, !query.is_empty(), collapsed, &mut rows);
    rows
}

/// ブランチ選択イベント
#[derive(Clone, Debug)]
pub(crate) enum BranchPanelEvent {
    /// ブランチ切り替えをリクエスト
    SwitchBranch { name: String },
    /// 新規ブランチ作成をリクエスト
    CreateBranch,
}

impl gpui::EventEmitter<BranchPanelEvent> for BranchPanelFeature {}

/// IntelliJ風のブランチ選択パネル
pub(crate) struct BranchPanelFeature {
    git: Entity<GitFeature>,
    focus_handle: FocusHandle,
    searchable: Entity<SearchInput>,
    open: bool,
    query: String,
    branches: Vec<Branch>,
    collapsed: HashSet<String>,
    selected: Option<String>,
    highlighted: Option<String>,
    pending_selection: Option<String>,
    context: Option<(WorkspaceId, String, String)>,
    generation: u64,
    loading: bool,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl BranchPanelFeature {
    pub(crate) fn new(
        git: Entity<GitFeature>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let searchable = cx.new(|cx| SearchInput::new("ブランチを検索…", cx));
        let subscriptions = vec![
            cx.subscribe(&searchable, |this, _, event, cx| {
                let SearchInputEvent::Changed(query) = event;
                this.query = query.clone();
                this.highlighted = None;
                cx.notify();
            }),
            cx.observe_in(&git, window, |this, git, _window, cx| {
                let context = git.read(cx).header_context();
                let message = git.read(cx).message.clone();

                // ブランチ切り替えが完了したら閉じる
                if this
                    .pending_selection
                    .as_deref()
                    .is_some_and(|name| context.as_ref().is_some_and(|c| c.2 == *name))
                {
                    this.pending_selection = None;
                    this.selected = context.as_ref().map(|c| c.2.clone());
                }

                // コンテキストが変更されたら再読み込み
                if this.context != context {
                    this.context = context.clone();
                    this.generation = this.generation.wrapping_add(1);
                    this.branches.clear();
                    this.loading = false;
                    this.error = None;
                    this.selected = context.as_ref().map(|c| c.2.clone());
                    if this.open {
                        this.reload(cx);
                    }
                } else if this.pending_selection.is_some() && !message.is_empty() {
                    this.error = Some(message);
                }
                cx.notify();
            }),
        ];

        let context = git.read(cx).header_context();
        let self_ = Self {
            git,
            focus_handle: cx.focus_handle(),
            searchable,
            open: true,
            query: String::new(),
            branches: Vec::new(),
            collapsed: HashSet::new(),
            selected: context.as_ref().map(|c| c.2.clone()),
            highlighted: None,
            pending_selection: None,
            context,
            generation: 0,
            loading: false,
            error: None,
            _subscriptions: subscriptions,
        };
        self_
    }

    /// ブランチ一覧を再読み込み
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.loading = true;
        self.error = None;
        let panel = cx.entity().downgrade();

        let result = self.git.update(cx, |git, cx| {
            git.load_header_branches(cx, move |result, cx| {
                let _ = panel.update(cx, |this, cx| {
                    if generation != this.generation {
                        return;
                    }
                    this.loading = false;
                    match result.and_then(|output| parse_branches(&output)) {
                        Ok(branches) => this.branches = branches,
                        Err(error) => {
                            this.branches.clear();
                            this.error = Some(error);
                        }
                    }
                    cx.notify();
                });
            })
        });

        if let Err(error) = result {
            self.loading = false;
            self.error = Some(error);
        }
        cx.notify();
    }

    /// ブランチを選択して切り替え
    fn choose(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.context.as_ref().is_some_and(|c| c.2 == name) {
            return;
        }
        if self.loading || !self.branches.iter().any(|b| b.name == name) {
            return;
        }
        self.pending_selection = Some(name.clone());
        self.error = None;
        self.git.update(cx, |git, cx| {
            git.execute_repository_action(
                "root".into(),
                GitAction::SwitchBranch { name },
                window,
                cx,
            )
        });
        cx.notify();
    }

    /// グループの折りたたみ状態を切り替え
    fn toggle_group(&mut self, path: String, cx: &mut Context<Self>) {
        if !self.collapsed.insert(path.clone()) {
            self.collapsed.remove(&path);
        }
        cx.notify();
    }

    /// 検索クエリをクリア
    fn clear_search(&mut self, cx: &mut Context<Self>) {
        self.query.clear();
        self.searchable
            .update(cx, |s, cx| s.set_text(String::new(), cx));
        self.highlighted = None;
        cx.notify();
    }

    /// すべてのグループを展開
    fn expand_all(&mut self, cx: &mut Context<Self>) {
        self.collapsed.clear();
        cx.notify();
    }
}

impl Focusable for BranchPanelFeature {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for BranchPanelFeature {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme::colors(cx);

        // ヘッダー部分
        let header = div()
            .id("branch-panel-header")
            .flex()
            .items_center()
            .justify_between()
            .px(tokens::spacing::XS)
            .py(tokens::spacing::XS)
            .border_b_1()
            .border_color(colors.border_default)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(Icon::new(IconName::Git))
                    .child(
                        div()
                            .text_size(tokens::typography::FONT_SM)
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(colors.text_primary)
                            .child("ブランチ"),
                    ),
            )
            .child(
                div().flex().items_center().gap_1().children([
                    // 新規ブランチボタン
                    crate::components::icon_button("branch-create", IconName::FilePlus, cx)
                        .on_click(cx.listener(|this, _, _, cx| {
                            cx.emit(BranchPanelEvent::CreateBranch);
                        })),
                    // 更新ボタン
                    crate::components::icon_button("branch-refresh", IconName::History, cx)
                        .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                ]),
            );

        // 検索入力
        let search = div()
            .id("branch-panel-search")
            .px(tokens::spacing::XS)
            .py(tokens::spacing::XS)
            .child(self.searchable.clone());

        // コンテンツ部分
        let content: gpui::AnyElement = if self.loading {
            div()
                .flex()
                .items_center()
                .justify_center()
                .py_4()
                .child(
                    div()
                        .text_size(tokens::typography::FONT_SM)
                        .text_color(colors.text_secondary)
                        .child("読み込み中…"),
                )
                .into_any_element()
        } else if self.branches.is_empty() && self.error.is_none() {
            panel_empty_state(
                "⌘B",
                "ブランチがありません",
                "Gitリポジトリを確認してください",
                cx,
            )
            .into_any_element()
        } else {
            // ブランチツリー
            let rows = build_branch_rows(&self.branches, &self.query, &self.collapsed);

            div()
                .id("branch-tree")
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .overflow_y_scroll()
                .px(tokens::spacing::XS)
                .py(tokens::spacing::XS)
                .when(rows.is_empty(), |this| {
                    this.child(
                        div().flex().items_center().justify_center().py_4().child(
                            div()
                                .text_size(tokens::typography::FONT_SM)
                                .text_color(colors.text_secondary)
                                .child("一致するブランチがありません"),
                        ),
                    )
                })
                .when(!rows.is_empty(), |this| {
                    this.children(rows.iter().enumerate().map(|(idx, row)| match row {
                        BranchRow::Group {
                            path,
                            label,
                            depth,
                            expanded,
                        } => {
                            let is_expanded = *expanded;
                            let path = path.clone();
                            div()
                                .id(("branch-group", idx))
                                .h(px(27.0))
                                .pl(px(6.0 + (*depth as f32) * 14.0))
                                .pr(tokens::spacing::XS)
                                .rounded(tokens::radius::CONTROL)
                                .flex()
                                .items_center()
                                .gap(tokens::spacing::XS)
                                .text_size(tokens::typography::FONT_SM)
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(colors.text_primary)
                                .child(
                                    div()
                                        .w(px(14.0))
                                        .flex_shrink_0()
                                        .text_color(colors.text_secondary)
                                        .child(if is_expanded { "⌄" } else { ">" }),
                                )
                                .child(
                                    div()
                                        .text_color(colors.text_secondary)
                                        .child(Icon::new(IconName::Files)),
                                )
                                .child(label.clone())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.toggle_group(path.clone(), cx);
                                }))
                                .into_any_element()
                        }
                        BranchRow::Branch {
                            branch,
                            label,
                            depth,
                        } => {
                            let name = branch.name.clone();
                            let is_selected = self.selected.as_deref() == Some(&name);
                            let is_highlighted = self.highlighted.as_deref() == Some(&name);

                            let row_bg = if is_selected {
                                colors.button_background_selected
                            } else if is_highlighted {
                                colors.button_background_hover
                            } else {
                                gpui::rgba(0x00000000)
                            };

                            div()
                                .id(("branch-item", idx))
                                .h(px(27.0))
                                .pl(px(6.0 + (*depth as f32) * 14.0))
                                .pr(tokens::spacing::XS)
                                .rounded(tokens::radius::CONTROL)
                                .bg(row_bg)
                                .flex()
                                .items_center()
                                .gap(tokens::spacing::XS)
                                .text_size(tokens::typography::FONT_SM)
                                .text_color(colors.text_primary)
                                .child(
                                    div()
                                        .w(px(14.0))
                                        .flex_shrink_0()
                                        .text_color(colors.text_secondary)
                                        .child(""),
                                )
                                .child(
                                    div()
                                        .text_color(if is_selected {
                                            colors.text_primary
                                        } else {
                                            colors.text_secondary
                                        })
                                        .child(Icon::new(IconName::Git)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .child(label.clone()),
                                )
                                .when(is_selected, |style| {
                                    style.child(
                                        div()
                                            .text_size(tokens::typography::FONT_XS)
                                            .text_color(colors.text_secondary)
                                            .child("現在"),
                                    )
                                })
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.choose(name.clone(), window, cx);
                                }))
                                .into_any_element()
                        }
                    }))
                })
                .into_any_element()
        };

        // エラー表示
        let error_text = self.error.clone();
        let error_view = error_text.as_ref().map(|error| {
            div()
                .id("branch-error")
                .mx(tokens::spacing::XS)
                .my(tokens::spacing::XS)
                .px(tokens::spacing::XS)
                .py(tokens::spacing::XS)
                .rounded(tokens::radius::CONTROL)
                .bg(gpui::rgba(0xff44441a))
                .border_1()
                .border_color(gpui::rgba(0xff44444d))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .text_color(colors.danger_text)
                                .child(Icon::new(IconName::Problems)),
                        )
                        .child(
                            div()
                                .text_size(tokens::typography::FONT_XS)
                                .text_color(colors.danger_text)
                                .child(error.clone()),
                        ),
                )
        });

        div()
            .id("branch-panel")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .bg(colors.background_primary)
            .key_context("branch-panel")
            .track_focus(&self.focus_handle)
            .child(header)
            .child(search)
            .child(content)
            .children(error_view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_build_rows_with_hierarchy() {
        let branches =
            parse_branches("* fix/menu abc1234\n  fix/headers/branch def5678\n  develop 123abcd\n")
                .unwrap();
        assert_eq!(branches.len(), 3);

        let collapsed = HashSet::new();
        let rows = build_branch_rows(&branches, "", &collapsed);
        assert!(!rows.is_empty());

        let collapsed = HashSet::from(["fix".into()]);
        let rows = build_branch_rows(&branches, "", &collapsed);
        assert_eq!(rows.len(), 2); // fixグループ(折りたたみ) + develop

        let rows = build_branch_rows(&branches, "HEADERS", &collapsed);
        assert_eq!(rows.len(), 3); // 検索時は展開
    }
}
