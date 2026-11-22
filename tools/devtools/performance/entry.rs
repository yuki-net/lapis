//! Performance 診断ログの読み取り用形式。
//!
//! Lapis / GPUI のUI性能を記録し、DevTool Performance タブで表示する。

/// Performance 診断ログの1エントリ。
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(crate) struct PerformanceEntry {
    /// モノトニックなシーケンス番号。
    pub sequence: u64,
    /// 発生時刻。
    pub occurred_at: std::time::Instant,
    /// ラベル (例: "Frame", "Draw", "UI")。
    pub label: String,
    /// フレーム番号 (オプション)。
    pub frame_number: Option<u64>,
    /// 処理時間。
    pub duration: std::time::Duration,
}
