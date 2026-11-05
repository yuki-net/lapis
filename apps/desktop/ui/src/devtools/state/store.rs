//! State 診断の bounded in-memory store。
//!
//! Snapshot の履歴を保持し、Watch 機能で変更履歴を追跡する。

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::SystemTime;

use gpui::{App, Global};

use super::snapshot::{DesktopStateSnapshot, StateNode};

/// Watch 変更履歴の上限。
const WATCH_HISTORY_MAX: usize = 500;

/// Watch で記録された変更イベント。
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(crate) struct WatchEvent {
    pub(crate) occurred_at: SystemTime,
    pub(crate) path: String,
    pub(crate) old_value: String,
    pub(crate) new_value: String,
}

/// State 診断の store。
#[derive(Default)]
pub(crate) struct StateStore {
    inner: Mutex<StateStoreInner>,
}

#[derive(Default)]
struct StateStoreInner {
    /// 最新の Snapshot
    current: Option<DesktopStateSnapshot>,
    /// Watch が有効かどうか
    watch_enabled: bool,
    /// Watch 変更履歴（bounded）
    watch_history: VecDeque<WatchEvent>,
}

impl Global for StateStore {}

pub(crate) fn init(cx: &mut App) {
    cx.set_global(StateStore::default());
}

pub(crate) fn store(cx: &App) -> &StateStore {
    cx.global::<StateStore>()
}

impl StateStore {
    /// 最新の Snapshot を取得する。
    pub(crate) fn current(&self) -> Option<DesktopStateSnapshot> {
        self.inner.lock().unwrap().current.clone()
    }

    /// Snapshot を更新する。Watch が有効なら差分を記録する。
    pub(crate) fn set_snapshot(&self, snapshot: DesktopStateSnapshot) {
        let mut inner = self.inner.lock().unwrap();
        if inner.watch_enabled
            && let Some(ref previous) = inner.current
        {
            let events = Self::diff_snapshots(previous, &snapshot);
            for event in events {
                if inner.watch_history.len() >= WATCH_HISTORY_MAX {
                    inner.watch_history.pop_front();
                }
                inner.watch_history.push_back(event);
            }
        }
        inner.current = Some(snapshot);
    }

    /// Watch を有効/無効にする。
    pub(crate) fn set_watch_enabled(&self, enabled: bool) {
        self.inner.lock().unwrap().watch_enabled = enabled;
    }

    /// Watch が有効かどうか。
    pub(crate) fn is_watch_enabled(&self) -> bool {
        self.inner.lock().unwrap().watch_enabled
    }

    /// Watch 変更履歴を取得する。
    pub(crate) fn watch_history(&self) -> Vec<WatchEvent> {
        self.inner
            .lock()
            .unwrap()
            .watch_history
            .iter()
            .cloned()
            .collect()
    }

    /// Store をクリアする。
    pub(crate) fn clear(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.current = None;
        inner.watch_history.clear();
    }

    /// 2つの Snapshot の差分から WatchEvent を生成する。
    fn diff_snapshots(old: &DesktopStateSnapshot, new: &DesktopStateSnapshot) -> Vec<WatchEvent> {
        let mut events = Vec::new();
        let occurred_at = new.captured_at;
        for old_node in &old.nodes {
            match new.nodes.iter().find(|n| n.name == old_node.name) {
                Some(new_node) => {
                    Self::diff_node(old_node, new_node, &old_node.name, occurred_at, &mut events);
                }
                None => events.push(WatchEvent {
                    occurred_at,
                    path: old_node.name.clone(),
                    old_value: "(present)".to_string(),
                    new_value: "(removed)".to_string(),
                }),
            }
        }
        for new_node in &new.nodes {
            if !old.nodes.iter().any(|n| n.name == new_node.name) {
                events.push(WatchEvent {
                    occurred_at,
                    path: new_node.name.clone(),
                    old_value: "(absent)".to_string(),
                    new_value: "(added)".to_string(),
                });
            }
        }
        events
    }

    fn diff_node(
        old: &StateNode,
        new: &StateNode,
        path: &str,
        occurred_at: SystemTime,
        events: &mut Vec<WatchEvent>,
    ) {
        Self::diff_node_entries(old, new, path, occurred_at, events);
        for old_child in &old.children {
            match new.children.iter().find(|c| c.name == old_child.name) {
                Some(new_child) => {
                    let child_path = format!("{}/{}", path, old_child.name);
                    Self::diff_node(old_child, new_child, &child_path, occurred_at, events);
                }
                None => events.push(WatchEvent {
                    occurred_at,
                    path: format!("{}/{}", path, old_child.name),
                    old_value: "(present)".to_string(),
                    new_value: "(removed)".to_string(),
                }),
            }
        }
        for new_child in &new.children {
            if !old.children.iter().any(|c| c.name == new_child.name) {
                events.push(WatchEvent {
                    occurred_at,
                    path: format!("{}/{}", path, new_child.name),
                    old_value: "(absent)".to_string(),
                    new_value: "(added)".to_string(),
                });
            }
        }
    }

    fn diff_node_entries(
        old: &StateNode,
        new: &StateNode,
        path: &str,
        occurred_at: SystemTime,
        events: &mut Vec<WatchEvent>,
    ) {
        for old_entry in &old.entries {
            match new.entries.iter().find(|e| e.key == old_entry.key) {
                Some(new_entry) if old_entry.value != new_entry.value => {
                    events.push(WatchEvent {
                        occurred_at,
                        path: format!("{}/{}", path, old_entry.key),
                        old_value: old_entry.value.clone(),
                        new_value: new_entry.value.clone(),
                    });
                }
                Some(_) => {}
                None => {
                    events.push(WatchEvent {
                        occurred_at,
                        path: format!("{}/{}", path, old_entry.key),
                        old_value: old_entry.value.clone(),
                        new_value: "(removed)".to_string(),
                    });
                }
            }
        }
        for new_entry in &new.entries {
            if !old.entries.iter().any(|entry| entry.key == new_entry.key) {
                events.push(WatchEvent {
                    occurred_at,
                    path: format!("{}/{}", path, new_entry.key),
                    old_value: "(added)".to_string(),
                    new_value: new_entry.value.clone(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devtools::state::snapshot::{StateEntry, StateSource};

    #[test]
    fn store_init_and_clear() {
        let store = StateStore::default();
        assert!(store.current().is_none());
        store.clear();
        assert!(store.current().is_none());
    }

    #[test]
    fn set_snapshot_updates_current() {
        let store = StateStore::default();
        let snapshot = DesktopStateSnapshot::default();
        store.set_snapshot(snapshot.clone());
        assert_eq!(store.current().unwrap().nodes.len(), snapshot.nodes.len());
    }

    #[test]
    fn watch_enabled_controls_diff() {
        let store = StateStore::default();

        let mut snap1 = DesktopStateSnapshot::default();
        snap1.nodes.push(super::super::snapshot::StateNode::leaf(
            "Test".to_string(),
            vec![super::super::snapshot::StateEntry {
                key: "value".to_string(),
                value: "old".to_string(),
                source: super::super::snapshot::StateSource::Desktop,
            }],
        ));
        store.set_snapshot(snap1);

        // Watch OFF では履歴を増やさない
        assert!(!store.is_watch_enabled());
        let mut snap2 = DesktopStateSnapshot::default();
        snap2.nodes.push(super::super::snapshot::StateNode::leaf(
            "Test".to_string(),
            vec![super::super::snapshot::StateEntry {
                key: "value".to_string(),
                value: "new".to_string(),
                source: super::super::snapshot::StateSource::Desktop,
            }],
        ));
        store.set_snapshot(snap2);
        assert!(store.watch_history().is_empty());

        // Watch ON にして再度変更
        store.set_watch_enabled(true);
        let mut snap3 = DesktopStateSnapshot::default();
        snap3.nodes.push(super::super::snapshot::StateNode::leaf(
            "Test".to_string(),
            vec![super::super::snapshot::StateEntry {
                key: "value".to_string(),
                value: "changed".to_string(),
                source: super::super::snapshot::StateSource::Desktop,
            }],
        ));
        store.set_snapshot(snap3);
        assert!(!store.watch_history().is_empty());
    }

    #[test]
    fn watch_records_nested_child_changes() {
        let store = StateStore::default();
        store.set_watch_enabled(true);

        let snapshot = |dirty: &str| {
            let mut snapshot = DesktopStateSnapshot::default();
            snapshot.nodes.push(StateNode {
                name: "Shell".into(),
                entries: Vec::new(),
                children: vec![StateNode {
                    name: "Tabs".into(),
                    entries: Vec::new(),
                    children: vec![StateNode::leaf(
                        "main.rs",
                        vec![StateEntry {
                            key: "dirty".into(),
                            value: dirty.into(),
                            source: StateSource::Desktop,
                        }],
                    )],
                }],
            });
            snapshot
        };

        store.set_snapshot(snapshot("false"));
        store.set_snapshot(snapshot("true"));

        let history = store.watch_history();
        assert!(history.iter().any(|event| {
            event.path == "Shell/Tabs/main.rs/dirty"
                && event.old_value == "false"
                && event.new_value == "true"
        }));
    }

    #[test]
    fn watch_records_added_and_removed_nodes() {
        let store = StateStore::default();
        store.set_watch_enabled(true);

        let snapshot = |tab: Option<&str>| {
            let mut snapshot = DesktopStateSnapshot::default();
            let children = tab
                .map(|name| vec![StateNode::leaf(name, Vec::new())])
                .unwrap_or_default();
            snapshot.nodes.push(StateNode {
                name: "Shell".into(),
                entries: Vec::new(),
                children: vec![StateNode {
                    name: "Tabs".into(),
                    entries: Vec::new(),
                    children,
                }],
            });
            snapshot
        };

        store.set_snapshot(snapshot(None));
        store.set_snapshot(snapshot(Some("main.rs")));
        store.set_snapshot(snapshot(None));

        let history = store.watch_history();
        assert!(history.iter().any(|event| {
            event.path == "Shell/Tabs/main.rs"
                && event.old_value == "(absent)"
                && event.new_value == "(added)"
        }));
        assert!(history.iter().any(|event| {
            event.path == "Shell/Tabs/main.rs"
                && event.old_value == "(present)"
                && event.new_value == "(removed)"
        }));
    }

    #[test]
    fn watch_history_is_bounded() {
        let store = StateStore::default();
        store.set_watch_enabled(true);

        for i in 0..WATCH_HISTORY_MAX + 100 {
            let mut snap = DesktopStateSnapshot::default();
            snap.nodes.push(super::super::snapshot::StateNode::leaf(
                "Counter".to_string(),
                vec![super::super::snapshot::StateEntry {
                    key: "count".to_string(),
                    value: i.to_string(),
                    source: super::super::snapshot::StateSource::Desktop,
                }],
            ));
            store.set_snapshot(snap);
        }

        let history = store.watch_history();
        assert!(history.len() <= WATCH_HISTORY_MAX);
    }
}
