//! State 診断の bounded in-memory store。
//!
//! Snapshot の履歴を保持し、Watch 機能で変更履歴を追跡する。

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::SystemTime;

use gpui::{App, Global};

use super::snapshot::DesktopStateSnapshot;

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
    /// 直前の Snapshot（Watch 差分用）
    previous: Option<DesktopStateSnapshot>,
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
        if inner.watch_enabled {
            if let Some(ref previous) = inner.current {
                let events = Self::diff_snapshots(previous, &snapshot);
                for event in events {
                    if inner.watch_history.len() >= WATCH_HISTORY_MAX {
                        inner.watch_history.pop_front();
                    }
                    inner.watch_history.push_back(event);
                }
            }
            inner.previous = inner.current.clone();
        }
        inner.current = Some(snapshot);
    }

    /// Watch を有効/無効にする。
    pub(crate) fn set_watch_enabled(&self, enabled: bool) {
        let mut inner = self.inner.lock().unwrap();
        if enabled && !inner.watch_enabled {
            // Watch を有効化した時点で現在値を基準にする
            inner.previous = inner.current.clone();
        }
        inner.watch_enabled = enabled;
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
        inner.previous = None;
        inner.watch_history.clear();
    }

    /// 2つの Snapshot の差分から WatchEvent を生成する。
    fn diff_snapshots(old: &DesktopStateSnapshot, new: &DesktopStateSnapshot) -> Vec<WatchEvent> {
        let mut events = Vec::new();
        for old_node in &old.nodes {
            if let Some(new_node) = new.nodes.iter().find(|n| n.name == old_node.name) {
                Self::diff_node_entries(old_node, new_node, &old_node.name, &mut events);
            }
        }
        events
    }

    fn diff_node_entries(
        old: &super::snapshot::StateNode,
        new: &super::snapshot::StateNode,
        path: &str,
        events: &mut Vec<WatchEvent>,
    ) {
        let now = SystemTime::now();
        for old_entry in &old.entries {
            if let Some(new_entry) = new.entries.iter().find(|e| e.key == old_entry.key)
                && old_entry.value != new_entry.value
            {
                events.push(WatchEvent {
                    occurred_at: now,
                    path: format!("{}/{}", path, old_entry.key),
                    old_value: old_entry.value.clone(),
                    new_value: new_entry.value.clone(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
