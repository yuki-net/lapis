//! 診断ログの読み取り用形式と、ログの発生源の区分。

/// 診断ログの発生源。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum DiagnosticSource {
    #[default]
    Desktop,
}

impl DiagnosticSource {
    /// 現在定義されているすべてのSource。
    #[allow(dead_code)]
    pub(crate) fn all() -> &'static [DiagnosticSource] {
        &[DiagnosticSource::Desktop]
    }

    /// Source の表示名。
    #[allow(dead_code)]
    pub(crate) fn title(self) -> &'static str {
        match self {
            DiagnosticSource::Desktop => "Desktop",
        }
    }
}

/// 診断ログのレベル。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub(crate) enum DiagnosticLevel {
    Trace,
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

impl DiagnosticLevel {
    /// レベルの表示名。
    #[allow(dead_code)]
    pub(crate) fn title(self) -> &'static str {
        match self {
            DiagnosticLevel::Trace => "TRACE",
            DiagnosticLevel::Debug => "DEBUG",
            DiagnosticLevel::Info => "INFO",
            DiagnosticLevel::Warn => "WARN",
            DiagnosticLevel::Error => "ERROR",
        }
    }

    /// tracing のレベルから変換する。
    pub(crate) fn from_tracing(level: tracing::Level) -> Self {
        match level {
            tracing::Level::ERROR => DiagnosticLevel::Error,
            tracing::Level::WARN => DiagnosticLevel::Warn,
            tracing::Level::INFO => DiagnosticLevel::Info,
            tracing::Level::DEBUG => DiagnosticLevel::Debug,
            tracing::Level::TRACE => DiagnosticLevel::Trace,
        }
    }
}

/// 診断ログの1エントリ。
///
/// `tracing::Event` を Lapis 側の読み取り用形式に変換したもので、
/// Console UI がこの型を直接表示する。
#[derive(Clone, Debug)]
pub(crate) struct DiagnosticLogEntry {
    /// モノトニックなシーケンス番号。
    pub sequence: u64,
    /// 発生時刻。
    pub occurred_at: std::time::SystemTime,
    /// 診断レベル。
    pub level: DiagnosticLevel,
    /// ログの発生源。
    pub source: DiagnosticSource,
    /// tracing target。
    pub target: String,
    /// メッセージ。
    pub message: String,
    /// 構造化フィールド（serde_json::Value として保持）。
    pub fields: serde_json::Value,
}
