//! State Snapshot の読み取り用型と、DesktopRoot からの取得関数。
//!
//! 正規状態を複製せず、診断用の read-only モデルへ変換する。

use std::time::SystemTime;

/// 診断 State の Source 区分。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StateSource {
    Desktop,
    /// 将来 Host 由来の診断状態を追加可能。
    #[allow(dead_code)]
    Host,
}

impl StateSource {
    #[allow(dead_code)]
    pub(crate) fn title(self) -> &'static str {
        match self {
            StateSource::Desktop => "Desktop",
            StateSource::Host => "Host",
        }
    }
}

/// Snapshot 内の 1つのキー値ペア。
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(crate) struct StateEntry {
    pub(crate) key: String,
    pub(crate) value: String,
    pub(crate) source: StateSource,
}

/// Snapshot のルート。Source ごとのツリーノードを保持する。
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(crate) struct DesktopStateSnapshot {
    pub(crate) captured_at: SystemTime,
    pub(crate) nodes: Vec<StateNode>,
}

impl Default for DesktopStateSnapshot {
    fn default() -> Self {
        Self {
            captured_at: SystemTime::UNIX_EPOCH,
            nodes: Vec::new(),
        }
    }
}

/// ツリー上の1つのノード。子ノードを持つことができる。
#[derive(Clone, Debug)]
pub(crate) struct StateNode {
    pub(crate) name: String,
    pub(crate) entries: Vec<StateEntry>,
    pub(crate) children: Vec<StateNode>,
}

impl StateNode {
    pub(crate) fn leaf(name: impl Into<String>, entries: Vec<StateEntry>) -> Self {
        Self {
            name: name.into(),
            entries,
            children: Vec::new(),
        }
    }
}

/// Shell の主要状態を診断用 read model として表現する。
#[derive(Clone, Debug)]
pub(crate) struct PanelSnapshot {
    pub(crate) id: String,
    pub(crate) active_tab: Option<String>,
    pub(crate) tab_count: usize,
    pub(crate) open: bool,
}

/// DesktopRoot から現在の状態を read-only で取得する。
///
/// 副作用なしで、メモリ上の状態を診断用 read model へ変換する。
/// ShellPresentation経由で公開されている状態のみを使用する。
pub(crate) fn capture_state(window: &gpui::Window, cx: &gpui::App) -> DesktopStateSnapshot {
    let root = window.root::<crate::root::DesktopRoot>();
    let mut snapshot = DesktopStateSnapshot {
        captured_at: SystemTime::now(),
        nodes: Vec::new(),
    };

    // Shell と Presentation — 借用を避けるために必要な値だけをcloneする
    if let Some(Some(root_entity)) = root {
        let shell = root_entity.read(cx).shell.clone();

        // Window 情報
        let window_node = StateNode {
            name: "Window".into(),
            entries: vec![
                StateEntry {
                    key: "active".into(),
                    value: window.is_window_active().to_string(),
                    source: StateSource::Desktop,
                },
                StateEntry {
                    key: "maximized".into(),
                    value: window.is_maximized().to_string(),
                    source: StateSource::Desktop,
                },
            ],
            children: Vec::new(),
        };
        snapshot.nodes.push(window_node);

        let shell_read = shell.read(cx);
        let shell_node = capture_shell(shell_read);
        snapshot.nodes.push(shell_node);
    }

    snapshot
}

fn capture_shell(shell: &crate::shell::main::Shell) -> StateNode {
    let shell_state = &shell.shell;
    let presentation = &shell.session;

    let focused_panel = shell_state.focused_panel.as_ref().map(|p| p.as_str());

    // Panel 一覧 — panel_ids() で ID を取得
    let mut panels = Vec::new();
    for panel_id in shell_state.layout.panel_ids() {
        if let Some(panel) = shell_state.layout.panel(&panel_id) {
            let active_tab = panel
                .active
                .as_ref()
                .and_then(|tab| tab.view_id().map(|v| v.as_str().to_string()));
            let panel_snapshot = PanelSnapshot {
                id: panel_id.as_str(),
                active_tab,
                tab_count: panel.tabs.len(),
                open: panel.open,
            };
            panels.push(panel_snapshot);
        }
    }

    let entries = vec![
        StateEntry {
            key: "focused_panel".into(),
            value: focused_panel.clone().unwrap_or_else(|| "none".into()),
            source: StateSource::Desktop,
        },
        StateEntry {
            key: "panel_count".into(),
            value: panels.len().to_string(),
            source: StateSource::Desktop,
        },
        StateEntry {
            key: "bottom_span_left".into(),
            value: shell_state
                .bottom_spans_left_layout(std::time::Instant::now())
                .to_string(),
            source: StateSource::Desktop,
        },
        StateEntry {
            key: "bottom_span_right".into(),
            value: shell_state
                .bottom_spans_right_layout(std::time::Instant::now())
                .to_string(),
            source: StateSource::Desktop,
        },
        StateEntry {
            key: "command_palette_open".into(),
            value: shell_state.command_palette_open.to_string(),
            source: StateSource::Desktop,
        },
        StateEntry {
            key: "workspace_root".into(),
            value: presentation
                .root
                .as_ref()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "none".into()),
            source: StateSource::Desktop,
        },
        StateEntry {
            key: "workspace_name".into(),
            value: presentation.workspace_name.clone(),
            source: StateSource::Desktop,
        },
        StateEntry {
            key: "active_document".into(),
            value: presentation
                .active
                .as_ref()
                .map(|id| id.to_string())
                .unwrap_or_else(|| "none".into()),
            source: StateSource::Desktop,
        },
        StateEntry {
            key: "document_count".into(),
            value: presentation.documents.len().to_string(),
            source: StateSource::Desktop,
        },
        StateEntry {
            key: "cursor".into(),
            value: format!("{},{}", presentation.cursor.0, presentation.cursor.1),
            source: StateSource::Desktop,
        },
    ];

    // Tab 子ノード
    let tab_children = presentation
        .documents
        .iter()
        .map(|doc| {
            StateNode::leaf(
                doc.display_name.clone(),
                vec![
                    StateEntry {
                        key: "id".into(),
                        value: doc.id.to_string(),
                        source: StateSource::Desktop,
                    },
                    StateEntry {
                        key: "path".into(),
                        value: doc
                            .path
                            .as_ref()
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_else(|| "(untitled)".into()),
                        source: StateSource::Desktop,
                    },
                    StateEntry {
                        key: "dirty".into(),
                        value: doc.dirty.to_string(),
                        source: StateSource::Desktop,
                    },
                    StateEntry {
                        key: "active".into(),
                        value: doc.active.to_string(),
                        source: StateSource::Desktop,
                    },
                    StateEntry {
                        key: "encoding".into(),
                        value: format!("{:?}", doc.encoding),
                        source: StateSource::Desktop,
                    },
                ],
            )
        })
        .collect();

    // Panel 子ノード
    let panel_children = panels
        .iter()
        .map(|p| {
            StateNode::leaf(
                &p.id,
                vec![
                    StateEntry {
                        key: "active_tab".into(),
                        value: p.active_tab.clone().unwrap_or_else(|| "none".into()),
                        source: StateSource::Desktop,
                    },
                    StateEntry {
                        key: "tab_count".into(),
                        value: p.tab_count.to_string(),
                        source: StateSource::Desktop,
                    },
                    StateEntry {
                        key: "open".into(),
                        value: p.open.to_string(),
                        source: StateSource::Desktop,
                    },
                ],
            )
        })
        .collect();

    StateNode {
        name: "Shell".into(),
        entries,
        children: vec![
            StateNode {
                name: "Tabs".into(),
                entries: Vec::new(),
                children: tab_children,
            },
            StateNode {
                name: "Panels".into(),
                entries: Vec::new(),
                children: panel_children,
            },
        ],
    }
}
