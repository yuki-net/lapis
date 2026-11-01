use super::GitFeature;
use crate::{
    components::{
        Icon, IconName, Menu, MenuEntry, MenuItemSpec, Popup, PopupPlacement, PopupScope,
        SearchInput, SearchInputEvent, header_button,
    },
    theme,
};
use gpui::{
    Context, Entity, FocusHandle, Focusable, KeyDownEvent, Render, Subscription, Window, div,
    prelude::*, px,
};
use lapis_editor_core::WorkspaceId;
use lapis_git::GitAction;
use std::collections::{BTreeMap, HashSet};

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

#[derive(Default)]
struct BranchTree {
    children: BTreeMap<String, BranchTree>,
    branch: Option<Branch>,
}
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

fn branch_rows(branches: &[Branch], query: &str, collapsed: &HashSet<String>) -> Vec<BranchRow> {
    let query = query.to_lowercase();
    let mut root = BranchTree::default();
    for branch in branches
        .iter()
        .filter(|branch| branch.name.to_lowercase().contains(&query))
    {
        let mut node = &mut root;
        for segment in branch.name.split('/') {
            node = node.children.entry(segment.into()).or_default();
        }
        node.branch = Some(branch.clone());
    }
    fn append(
        node: BranchTree,
        parent: &str,
        depth: usize,
        query: bool,
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
                let expanded = query || !collapsed.contains(&path);
                rows.push(BranchRow::Group {
                    path: path.clone(),
                    label,
                    depth,
                    expanded,
                });
                if expanded {
                    append(child, &path, depth + 1, query, collapsed, rows);
                }
            }
        }
    }
    let mut rows = Vec::new();
    append(root, "", 0, !query.is_empty(), collapsed, &mut rows);
    rows
}

pub(crate) struct BranchPicker {
    git: Entity<GitFeature>,
    input: Entity<SearchInput>,
    open: bool,
    query: String,
    branches: Vec<Branch>,
    collapsed: HashSet<String>,
    highlighted: Option<String>,
    pending_selection: Option<String>,
    context: Option<(WorkspaceId, String, String)>,
    generation: u64,
    loading: bool,
    error: Option<String>,
    previous_focus: Option<FocusHandle>,
    _subscriptions: Vec<Subscription>,
}

impl BranchPicker {
    pub(crate) fn new(
        git: Entity<GitFeature>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| SearchInput::new("ブランチを検索…", cx));
        let subscriptions =
            vec![
                cx.subscribe(&input, |this, _, event, cx| {
                    let SearchInputEvent::Changed(query) = event;
                    this.query = query.clone();
                    this.highlighted = None;
                    cx.notify();
                }),
                cx.observe_in(&git, window, |this, git, window, cx| {
                    let context = git.read(cx).header_context();
                    let message = git.read(cx).message.clone();
                    if this.pending_selection.as_deref().is_some_and(|name| {
                        context.as_ref().is_some_and(|context| context.2 == name)
                    }) {
                        this.dismiss(window, cx);
                    }
                    if this.context != context {
                        this.context = context;
                        this.generation = this.generation.wrapping_add(1);
                        this.branches.clear();
                        this.loading = false;
                        this.error = None;
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
        Self {
            git,
            input,
            open: false,
            query: String::new(),
            branches: Vec::new(),
            collapsed: HashSet::new(),
            highlighted: None,
            pending_selection: None,
            context,
            generation: 0,
            loading: false,
            error: None,
            previous_focus: None,
            _subscriptions: subscriptions,
        }
    }

    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = false;
        self.generation = self.generation.wrapping_add(1);
        self.loading = false;
        self.pending_selection = None;
        if let Some(focus) = self.previous_focus.take() {
            window.focus(&focus);
        }
        cx.notify();
    }
    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.dismiss(window, cx);
            return;
        }
        self.open = true;
        self.previous_focus = window.focused(cx);
        self.error = None;
        self.context = self.git.read(cx).header_context();
        self.highlighted = self.context.as_ref().map(|context| context.2.clone());
        self.input
            .update(cx, |input, cx| input.set_text(String::new(), cx));
        self.reload(cx);
        window.focus(&self.input.focus_handle(cx));
        cx.notify();
    }
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.loading = true;
        self.error = None;
        let picker = cx.entity().downgrade();
        let result = self.git.update(cx, |git, cx| {
            git.load_header_branches(cx, move |result, cx| {
                let _ = picker.update(cx, |this, cx| {
                    if !this.open || generation != this.generation {
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
    fn choose(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .context
            .as_ref()
            .is_some_and(|context| context.2 == name)
        {
            self.dismiss(window, cx);
            return;
        }
        if self.loading || !self.branches.iter().any(|branch| branch.name == name) {
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
    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        match event.keystroke.key.as_str() {
            "escape" => self.dismiss(window, cx),
            "enter" => {
                if let Some(name) = self.highlighted.clone() {
                    self.choose(name, window, cx);
                }
            }
            "up" | "down" => {
                let names = branch_rows(&self.branches, &self.query, &self.collapsed)
                    .into_iter()
                    .filter_map(|row| match row {
                        BranchRow::Branch { branch, .. } => Some(branch.name),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                if names.is_empty() {
                    return;
                }
                let current = names
                    .iter()
                    .position(|name| Some(name) == self.highlighted.as_ref());
                let next = if event.keystroke.key == "up" {
                    current
                        .unwrap_or(0)
                        .checked_sub(1)
                        .unwrap_or(names.len() - 1)
                } else {
                    current.map_or(0, |index| (index + 1) % names.len())
                };
                self.highlighted = Some(names[next].clone());
                cx.notify();
            }
            _ => return,
        }
        cx.stop_propagation();
    }
    fn spec(
        id: impl Into<gpui::SharedString>,
        label: impl Into<gpui::SharedString>,
        icon: IconName,
    ) -> MenuItemSpec {
        MenuItemSpec {
            id: id.into(),
            label: label.into(),
            shortcut: None,
            icon: Some(icon),
            enabled: true,
            selected: false,
            indent: 0,
            submenu: false,
            separator_before: false,
        }
    }
    fn panel(&self, window: &Window, cx: &mut Context<Self>) -> Menu {
        let mut entries = Vec::new();
        let mut create = Self::spec("branch-create", "新規ブランチ…", IconName::FilePlus);
        create.enabled = self.context.is_some() && !self.loading;
        entries.push(MenuEntry::new(create, cx, |row| {
            row.on_click(cx.listener(|this, _, window, cx| {
                this.dismiss(window, cx);
                this.git.update(cx, |git, cx| git.header_create_branch(cx));
            }))
            .into_any_element()
        }));
        let mut section = Self::spec("branch-local", "ローカルブランチ", IconName::Git);
        section.enabled = false;
        section.separator_before = true;
        entries.push(MenuEntry::new(section, cx, |row| row.into_any_element()));
        let rows = branch_rows(&self.branches, &self.query, &self.collapsed);
        if self.loading || rows.is_empty() {
            let mut status = Self::spec(
                "branch-status",
                if self.loading {
                    "読み込み中…"
                } else {
                    "一致するブランチがありません"
                },
                IconName::Search,
            );
            status.enabled = false;
            entries.push(MenuEntry::new(status, cx, |row| row.into_any_element()));
        }
        for row in rows {
            match row {
                BranchRow::Group {
                    path,
                    label,
                    depth,
                    expanded,
                } => {
                    let mut spec = Self::spec(
                        format!("branch-group-{path}"),
                        label,
                        if expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        },
                    );
                    spec.indent = depth;
                    entries.push(MenuEntry::new(spec, cx, |row| {
                        row.on_click(cx.listener(move |this, _, _, cx| {
                            if !this.collapsed.insert(path.clone()) {
                                this.collapsed.remove(&path);
                            }
                            cx.notify();
                        }))
                        .into_any_element()
                    }));
                }
                BranchRow::Branch {
                    branch,
                    label,
                    depth,
                } => {
                    let name = branch.name;
                    let mut spec = Self::spec(format!("branch-{name}"), label, IconName::Git);
                    spec.indent = depth;
                    spec.enabled = !self.loading;
                    spec.selected = self.highlighted.as_ref().map_or_else(
                        || {
                            self.context
                                .as_ref()
                                .is_some_and(|context| context.2 == name)
                        },
                        |selected| selected == &name,
                    );
                    spec.shortcut = Some(
                        if self
                            .context
                            .as_ref()
                            .is_some_and(|context| context.2 == name)
                        {
                            format!("現在 · {}", branch.oid)
                        } else {
                            branch.oid
                        }
                        .into(),
                    );
                    let hover_name = name.clone();
                    entries.push(MenuEntry::new(spec, cx, |row| {
                        row.on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            if *hovered && this.highlighted.as_ref() != Some(&hover_name) {
                                this.highlighted = Some(hover_name.clone());
                                cx.notify();
                            }
                        }))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.choose(name.clone(), window, cx)
                        }))
                        .into_any_element()
                    }));
                }
            }
        }
        let mut all = Self::spec("branch-all", "すべてのローカルブランチ", IconName::Files);
        all.separator_before = true;
        entries.push(MenuEntry::new(all, cx, |row| {
            row.on_click(cx.listener(|this, _, _, cx| {
                this.collapsed.clear();
                this.input
                    .update(cx, |input, cx| input.set_text(String::new(), cx));
                cx.notify();
            }))
            .into_any_element()
        }));
        let mut remote = Self::spec("branch-remote", "リモートブランチ", IconName::Git);
        remote.enabled = false;
        remote.shortcut = Some("未対応".into());
        entries.push(MenuEntry::new(remote, cx, |row| row.into_any_element()));
        let refresh = Self::spec("branch-refresh", "ブランチ一覧を更新", IconName::History);
        entries.push(MenuEntry::new(refresh, cx, |row| {
            row.on_click(cx.listener(|this, _, _, cx| this.reload(cx)))
                .into_any_element()
        }));
        if let Some(error) = &self.error {
            let mut spec = Self::spec("branch-error", error.clone(), IconName::Problems);
            spec.enabled = false;
            spec.separator_before = true;
            entries.push(MenuEntry::new(spec, cx, |row| {
                row.text_color(theme::colors(cx).danger_text)
                    .into_any_element()
            }));
        }
        Menu::new("branch-menu", entries)
            .debug_selector("branch-menu-panel")
            .header(div().m_2().child(self.input.clone()))
            .min_width(px(320.))
            .max_height(px(450.).min((window.viewport_size().height - px(56.)).max(px(80.))))
    }
}

impl Render for BranchPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scope = PopupScope::default();
        let branch = self
            .context
            .as_ref()
            .map_or("ブランチ", |context| context.2.as_str());
        let trigger = header_button(
            "header-branch",
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(Icon::new(IconName::Git))
                .child(branch.to_owned())
                .child(Icon::new(IconName::ChevronDown).size(px(12.))),
            cx,
        )
        .debug_selector(|| "branch-menu-trigger".into())
        .when(self.open, |button| {
            button.bg(theme::colors(cx).button_background_focused)
        })
        .on_click(cx.listener(|this, _, window, cx| this.toggle(window, cx)));
        div().on_key_down(cx.listener(Self::key_down)).child(
            Popup::new(trigger, PopupPlacement::BelowStart, scope)
                .on_dismiss(cx.listener(|this, _: &(), window, cx| this.dismiss(window, cx)))
                .when(self.open, |popup| popup.content(self.panel(window, cx))),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_branches_parse_group_collapse_and_search_without_losing_full_names() {
        let branches =
            parse_branches("* fix/menu abc1234\n  fix/headers/branch def5678\n  develop 123abcd\n")
                .unwrap();
        let collapsed = HashSet::from(["fix".into()]);
        let rows = branch_rows(&branches, "", &collapsed);
        assert_eq!(rows.len(), 2);
        let rows = branch_rows(&branches, "HEADERS", &collapsed);
        assert_eq!(rows.len(), 3);
        assert!(
            matches!(&rows[2], BranchRow::Branch { branch, depth: 2, .. } if branch.name == "fix/headers/branch")
        );
        assert!(branch_rows(&branches, "missing", &collapsed).is_empty());
        assert!(parse_branches("not a valid branch row").is_err());
    }
}
