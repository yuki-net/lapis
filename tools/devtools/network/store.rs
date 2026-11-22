//! Network 診断ログのインメモリ bounded store。
//!
//! VecDeque ベースのリングバッファで、上限を超えると古いエントリを削除する。
//! GPUI の Global としてアプリ全体で共有され、DevTool Network がこれを読み取る。

use std::collections::VecDeque;
use std::sync::Mutex;

use gpui::App;

use super::entry::NetworkEntry;

/// Network 診断ログ store の最大件数。
#[allow(dead_code)]
const MAX_ENTRIES: usize = 5_000;

/// Network 診断ログ store。
#[derive(Default)]
#[allow(dead_code)]
pub(crate) struct NetworkLogStore {
    entries: Mutex<VecDeque<NetworkEntry>>,
    sequence: Mutex<u64>,
    recording: Mutex<bool>,
}
impl gpui::Global for NetworkLogStore {}

impl NetworkLogStore {
    /// Record 状態を設定する。
    pub(crate) fn set_recording(&self, recording: bool) {
        *self.recording.lock().unwrap() = recording;
    }

    /// 現在 Record 中かどうか。
    pub(crate) fn is_recording(&self) -> bool {
        *self.recording.lock().unwrap()
    }

    /// エントリを追加する。Record 中のみ有効。
    pub(crate) fn push(&self, entry: NetworkEntry) {
        let recording = *self.recording.lock().unwrap();
        if !recording {
            return;
        }
        let mut entries = self.entries.lock().unwrap();
        entries.push_back(entry);
        if entries.len() > MAX_ENTRIES {
            entries.pop_front();
        }
    }

    /// Request の開始時刻を記録する。
    #[allow(dead_code)]
    pub(crate) fn record_request_start(&self, request_id: String, method: String) -> u64 {
        let seq = self.next_sequence();
        let entry = NetworkEntry {
            sequence: seq,
            occurred_at: std::time::Instant::now(),
            network_type: super::entry::NetworkType::Request,
            transport: super::entry::NetworkTransport::Embedded,
            method,
            request_id: Some(request_id),
            status: super::entry::NetworkStatus::Pending,
            duration: None,
        };
        self.push(entry);
        seq
    }

    /// Request の終了を記録し、duration を計算する。
    #[allow(dead_code)]
    pub(crate) fn record_request_end(
        &self,
        request_id: &str,
        status: super::entry::NetworkStatus,
        duration: std::time::Duration,
    ) {
        let mut entries = self.entries.lock().unwrap();
        // 対応する Request エントリを探して更新
        for entry in entries.iter_mut().rev() {
            if entry.request_id.as_deref() == Some(request_id)
                && entry.status == super::entry::NetworkStatus::Pending
            {
                entry.status = status;
                entry.duration = Some(duration);
                break;
            }
        }
    }

    /// Event を記録する。
    #[allow(dead_code)]
    pub(crate) fn record_event(&self, method: String) {
        let seq = self.next_sequence();
        let entry = NetworkEntry {
            sequence: seq,
            occurred_at: std::time::Instant::now(),
            network_type: super::entry::NetworkType::Event,
            transport: super::entry::NetworkTransport::Embedded,
            method,
            request_id: None,
            status: super::entry::NetworkStatus::Ok,
            duration: None,
        };
        self.push(entry);
    }

    /// 現在の全エントリをシーケンス順で返す。
    pub(crate) fn entries(&self) -> Vec<NetworkEntry> {
        let entries = self.entries.lock().unwrap();
        entries.iter().cloned().collect()
    }

    /// store をクリアする。
    pub(crate) fn clear(&self) {
        let mut entries = self.entries.lock().unwrap();
        entries.clear();
    }

    /// 次のシーケンス番号を取得する。
    fn next_sequence(&self) -> u64 {
        let mut seq = self.sequence.lock().unwrap();
        *seq += 1;
        *seq
    }
}

/// Network 診断ログ store を GPUI の Global として登録する。
pub(crate) fn init(cx: &mut App) {
    cx.set_global(NetworkLogStore::default());
}

/// Network 診断ログ store にアクセスする（読み取り）。
pub(crate) fn store(cx: &App) -> &NetworkLogStore {
    cx.global::<NetworkLogStore>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devtools::network::entry::{NetworkStatus, NetworkTransport, NetworkType};

    fn make_request_entry(seq: u64, request_id: &str, method: &str) -> NetworkEntry {
        NetworkEntry {
            sequence: seq,
            occurred_at: std::time::Instant::now(),
            network_type: NetworkType::Request,
            transport: NetworkTransport::Embedded,
            method: method.to_string(),
            request_id: Some(request_id.to_string()),
            status: NetworkStatus::Pending,
            duration: None,
        }
    }

    fn make_event_entry(seq: u64, method: &str) -> NetworkEntry {
        NetworkEntry {
            sequence: seq,
            occurred_at: std::time::Instant::now(),
            network_type: NetworkType::Event,
            transport: NetworkTransport::Embedded,
            method: method.to_string(),
            request_id: None,
            status: NetworkStatus::Ok,
            duration: None,
        }
    }

    #[test]
    fn store_does_not_record_when_off() {
        let store = NetworkLogStore::default();
        assert!(!store.is_recording());
        store.push(make_request_entry(1, "req-1", "workspace.list"));
        assert!(store.entries().is_empty());
    }

    #[test]
    fn store_records_when_on() {
        let store = NetworkLogStore::default();
        store.set_recording(true);
        assert!(store.is_recording());
        store.push(make_request_entry(1, "req-1", "workspace.list"));
        let entries = store.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].method, "workspace.list");
    }

    #[test]
    fn store_clears_entries() {
        let store = NetworkLogStore::default();
        store.set_recording(true);
        store.push(make_request_entry(1, "req-1", "workspace.list"));
        store.push(make_event_entry(2, "document.delta"));
        assert_eq!(store.entries().len(), 2);
        store.clear();
        assert!(store.entries().is_empty());
    }

    #[test]
    fn store_enforces_max_entries() {
        let store = NetworkLogStore::default();
        store.set_recording(true);
        for i in 0..MAX_ENTRIES + 100 {
            store.push(make_request_entry(i as u64, &format!("req-{}", i), "test"));
        }
        let entries = store.entries();
        assert_eq!(entries.len(), MAX_ENTRIES);
        assert_eq!(entries[0].sequence, 100);
    }

    #[test]
    fn record_request_start_and_end() {
        let store = NetworkLogStore::default();
        store.set_recording(true);
        store.record_request_start("req-1".to_string(), "workspace.list".to_string());
        let entries = store.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, NetworkStatus::Pending);
        assert!(entries[0].duration.is_none());

        store.record_request_end(
            "req-1",
            NetworkStatus::Ok,
            std::time::Duration::from_millis(42),
        );
        let entries = store.entries();
        assert_eq!(entries[0].status, NetworkStatus::Ok);
        assert_eq!(
            entries[0].duration,
            Some(std::time::Duration::from_millis(42))
        );
    }

    #[test]
    fn record_event() {
        let store = NetworkLogStore::default();
        store.set_recording(true);
        store.record_event("document.delta".to_string());
        let entries = store.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].network_type, NetworkType::Event);
        assert_eq!(entries[0].method, "document.delta");
    }

    #[test]
    fn concurrent_requests_do_not_interfere() {
        let store = NetworkLogStore::default();
        store.set_recording(true);
        store.record_request_start("req-1".to_string(), "workspace.list".to_string());
        store.record_request_start("req-2".to_string(), "document.open".to_string());
        store.record_request_start("req-3".to_string(), "document.save".to_string());

        let entries = store.entries();
        assert_eq!(entries.len(), 3);

        store.record_request_end(
            "req-2",
            NetworkStatus::Ok,
            std::time::Duration::from_millis(10),
        );
        store.record_request_end(
            "req-1",
            NetworkStatus::Error,
            std::time::Duration::from_millis(20),
        );

        let entries = store.entries();
        assert_eq!(entries[0].status, NetworkStatus::Error);
        assert_eq!(entries[0].method, "workspace.list");
        assert_eq!(entries[1].status, NetworkStatus::Ok);
        assert_eq!(entries[1].method, "document.open");
        assert_eq!(entries[2].status, NetworkStatus::Pending);
        assert_eq!(entries[2].method, "document.save");
    }
}
