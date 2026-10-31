//! 診断ログのインメモリ bounded store。
//!
//! VecDeque ベースのリングバッファで、上限を超えると古いエントリを削除する。
//! GPUI の Global としてアプリ全体で共有され、DevTool Console がこれを読み取る。

use std::collections::VecDeque;
use std::sync::Mutex;

use gpui::App;

use super::entry::DiagnosticLogEntry;

/// 診断ログ store の最大件数。
const MAX_ENTRIES: usize = 10_000;

/// 診断ログ store。
///
/// Mutex で保護された VecDeque で、tracing Layer から非同期に書き込み、
/// Console UI から同期的に読み取る。
#[derive(Default)]
pub(crate) struct DiagnosticLogStore {
    entries: Mutex<VecDeque<DiagnosticLogEntry>>,
    sequence: Mutex<u64>,
}
impl gpui::Global for DiagnosticLogStore {}

impl DiagnosticLogStore {
    /// エントリを追加する。上限を超えると古いエントリを削除する。
    pub(crate) fn push(&self, entry: DiagnosticLogEntry) {
        let mut entries = self.entries.lock().unwrap();
        entries.push_back(entry);
        if entries.len() > MAX_ENTRIES {
            entries.pop_front();
        }
    }

    /// 現在の全エントリをシーケンス順で返す。
    pub(crate) fn entries(&self) -> Vec<DiagnosticLogEntry> {
        let entries = self.entries.lock().unwrap();
        entries.iter().cloned().collect()
    }

    /// store をクリアする。
    pub(crate) fn clear(&self) {
        let mut entries = self.entries.lock().unwrap();
        entries.clear();
    }

    /// 現在のエントリ数を返す。
    #[allow(dead_code)]
    pub(crate) fn len(&self) -> usize {
        let entries = self.entries.lock().unwrap();
        entries.len()
    }

    /// 次のシーケンス番号を取得する。
    pub(crate) fn next_sequence(&self) -> u64 {
        let mut seq = self.sequence.lock().unwrap();
        *seq += 1;
        *seq
    }
}

/// 診断ログ store を GPUI の Global として登録する。
pub(crate) fn init(cx: &mut App) {
    cx.set_global(DiagnosticLogStore::default());
}

/// 診断ログ store にアクセスする（読み取り）。
pub(crate) fn store(cx: &App) -> &DiagnosticLogStore {
    cx.global::<DiagnosticLogStore>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devtools::console::entry::{DiagnosticLevel, DiagnosticSource};
    use std::time::SystemTime;

    fn make_entry(seq: u64, level: DiagnosticLevel) -> DiagnosticLogEntry {
        DiagnosticLogEntry {
            sequence: seq,
            occurred_at: SystemTime::now(),
            level,
            source: DiagnosticSource::Desktop,
            target: "test".to_string(),
            message: format!("message {}", seq),
            fields: serde_json::json!({}),
        }
    }

    #[test]
    fn store_pushes_and_returns_entries() {
        let store = DiagnosticLogStore::default();
        let e1 = make_entry(1, DiagnosticLevel::Info);
        let e2 = make_entry(2, DiagnosticLevel::Error);
        store.push(e1.clone());
        store.push(e2.clone());

        let entries = store.entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].sequence, 1);
        assert_eq!(entries[1].sequence, 2);
    }

    #[test]
    fn store_clears_entries() {
        let store = DiagnosticLogStore::default();
        store.push(make_entry(1, DiagnosticLevel::Info));
        store.push(make_entry(2, DiagnosticLevel::Info));
        assert_eq!(store.len(), 2);

        store.clear();
        assert_eq!(store.len(), 0);
        assert!(store.entries().is_empty());
    }

    #[test]
    fn store_enforces_max_entries() {
        let store = DiagnosticLogStore::default();
        for i in 0..MAX_ENTRIES + 100 {
            store.push(make_entry(i as u64, DiagnosticLevel::Info));
        }

        let entries = store.entries();
        assert_eq!(entries.len(), MAX_ENTRIES);
        // 最も古いエントリは削除されている
        assert_eq!(entries[0].sequence, 100);
        assert_eq!(entries[MAX_ENTRIES - 1].sequence, (MAX_ENTRIES + 99) as u64);
    }

    #[test]
    fn store_next_sequence_is_monotonic() {
        let store = DiagnosticLogStore::default();
        let s1 = store.next_sequence();
        let s2 = store.next_sequence();
        assert_eq!(s1 + 1, s2);
    }

    #[allow(clippy::assertions_on_constants)]
    #[test]
    fn bounded_store_limit_constant_is_reasonable() {
        // 巨大なメモリ使用を避けるため、上限は合理的な範囲内であること
        assert!(MAX_ENTRIES > 100);
        assert!(MAX_ENTRIES < 1_000_000);
    }
}
