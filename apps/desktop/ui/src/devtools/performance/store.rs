//! Performance 診断ログのインメモリ bounded store。
//!
//! VecDeque ベースのリングバッファで、上限を超えると古いエントリを削除する。
//! GPUI の Global としてアプリ全体で共有され、DevTool Performance がこれを読み取る。

use std::collections::VecDeque;
use std::sync::Mutex;

use gpui::App;

use super::entry::PerformanceEntry;

/// Performance 診断ログ store の最大件数。
#[allow(dead_code)]
const MAX_ENTRIES: usize = 2_000;

/// Slow frame の閾値 (60Hz 前提の 16.67ms)。
/// これは厳密なディスプレイリフレッシュレートではなく、初期閾値として使用する。
#[allow(dead_code)]
pub(crate) const SLOW_FRAME_THRESHOLD_MS: f64 = 16.67;

/// Performance 診断ログ store。
#[derive(Default)]
#[allow(dead_code)]
pub(crate) struct PerformanceLogStore {
    entries: Mutex<VecDeque<PerformanceEntry>>,
    sequence: Mutex<u64>,
    recording: Mutex<bool>,
}
impl gpui::Global for PerformanceLogStore {}

impl PerformanceLogStore {
    /// Record 状態を設定する。
    pub(crate) fn set_recording(&self, recording: bool) {
        *self.recording.lock().unwrap() = recording;
    }

    /// 現在 Record 中かどうか。
    pub(crate) fn is_recording(&self) -> bool {
        *self.recording.lock().unwrap()
    }

    /// エントリを追加する。Record 中のみ有効。
    pub(crate) fn push(&self, entry: PerformanceEntry) {
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

    /// フレームの計測を記録する。
    #[allow(dead_code)]
    pub(crate) fn record_frame(&self, frame_number: u64, duration: std::time::Duration) {
        let seq = self.next_sequence();
        let entry = PerformanceEntry {
            sequence: seq,
            occurred_at: std::time::Instant::now(),
            label: "Frame".to_string(),
            frame_number: Some(frame_number),
            duration,
        };
        self.push(entry);
    }

    /// 任意の計測を記録する。
    #[allow(dead_code)]
    pub(crate) fn record_span(
        &self,
        label: String,
        frame_number: Option<u64>,
        duration: std::time::Duration,
    ) {
        let seq = self.next_sequence();
        let entry = PerformanceEntry {
            sequence: seq,
            occurred_at: std::time::Instant::now(),
            label,
            frame_number,
            duration,
        };
        self.push(entry);
    }

    /// 現在の全エントリをシーケンス順で返す。
    pub(crate) fn entries(&self) -> Vec<PerformanceEntry> {
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

/// Performance 診断ログ store を GPUI の Global として登録する。
pub(crate) fn init(cx: &mut App) {
    cx.set_global(PerformanceLogStore::default());
}

/// Performance 診断ログ store にアクセスする（読み取り）。
pub(crate) fn store(cx: &App) -> &PerformanceLogStore {
    cx.global::<PerformanceLogStore>()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(seq: u64, label: &str, duration_ms: u64) -> PerformanceEntry {
        PerformanceEntry {
            sequence: seq,
            occurred_at: std::time::Instant::now(),
            label: label.to_string(),
            frame_number: None,
            duration: std::time::Duration::from_millis(duration_ms),
        }
    }

    #[test]
    fn store_does_not_record_when_off() {
        let store = PerformanceLogStore::default();
        assert!(!store.is_recording());
        store.push(make_entry(1, "Frame", 10));
        assert!(store.entries().is_empty());
    }

    #[test]
    fn store_records_when_on() {
        let store = PerformanceLogStore::default();
        store.set_recording(true);
        assert!(store.is_recording());
        store.push(make_entry(1, "Frame", 10));
        let entries = store.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].label, "Frame");
    }

    #[test]
    fn store_clears_entries() {
        let store = PerformanceLogStore::default();
        store.set_recording(true);
        store.push(make_entry(1, "Frame", 10));
        store.push(make_entry(2, "Draw", 5));
        assert_eq!(store.entries().len(), 2);
        store.clear();
        assert!(store.entries().is_empty());
    }

    #[test]
    fn store_enforces_max_entries() {
        let store = PerformanceLogStore::default();
        store.set_recording(true);
        for i in 0..MAX_ENTRIES + 100 {
            store.push(make_entry(i as u64, "test", 1));
        }
        let entries = store.entries();
        assert_eq!(entries.len(), MAX_ENTRIES);
        assert_eq!(entries[0].sequence, 100);
    }

    #[test]
    fn record_frame() {
        let store = PerformanceLogStore::default();
        store.set_recording(true);
        store.record_frame(1, std::time::Duration::from_millis(12));
        let entries = store.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].label, "Frame");
        assert_eq!(entries[0].frame_number, Some(1));
        assert_eq!(entries[0].duration, std::time::Duration::from_millis(12));
    }

    #[test]
    fn record_span() {
        let store = PerformanceLogStore::default();
        store.set_recording(true);
        store.record_span(
            "Draw".to_string(),
            Some(1),
            std::time::Duration::from_millis(5),
        );
        let entries = store.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].label, "Draw");
        assert_eq!(entries[0].frame_number, Some(1));
    }

    #[test]
    fn slow_frame_threshold() {
        // 閾値が期待通りであることを確認
        const {
            assert!(24.8 > SLOW_FRAME_THRESHOLD_MS);
        }
        const {
            assert!(11.4 < SLOW_FRAME_THRESHOLD_MS);
        }
    }
}
