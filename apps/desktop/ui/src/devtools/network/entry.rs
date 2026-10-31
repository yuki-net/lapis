//! Network 診断ログの読み取り用形式。
//!
//! Lapis の論理的な通信操作（Request/Response/Event）を観測し、
//! Network UI がこの型を直接表示する。

/// 通信のタイプ。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[allow(dead_code)]
pub(crate) enum NetworkType {
    #[default]
    Request,
    Event,
}

impl NetworkType {
    /// タイプの表示名。
    #[allow(dead_code)]
    pub(crate) fn title(self) -> &'static str {
        match self {
            NetworkType::Request => "Request",
            NetworkType::Event => "Event",
        }
    }
}

/// 通信の結果状態。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[allow(dead_code)]
pub(crate) enum NetworkStatus {
    #[default]
    Pending,
    Ok,
    Error,
}

impl NetworkStatus {
    /// 状態の表示名。
    #[allow(dead_code)]
    pub(crate) fn title(self) -> &'static str {
        match self {
            NetworkStatus::Pending => "Pending",
            NetworkStatus::Ok => "OK",
            NetworkStatus::Error => "Error",
        }
    }
}

/// トランスポートの種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[allow(dead_code)]
pub(crate) enum NetworkTransport {
    #[default]
    Embedded,
    Remote,
}

impl NetworkTransport {
    /// トランスポートの表示名。
    #[allow(dead_code)]
    pub(crate) fn title(self) -> &'static str {
        match self {
            NetworkTransport::Embedded => "Embedded",
            NetworkTransport::Remote => "Remote",
        }
    }
}

/// Network 診断ログの1エントリ。
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(crate) struct NetworkEntry {
    /// モノトニックなシーケンス番号。
    pub sequence: u64,
    /// 発生時刻。
    pub occurred_at: std::time::Instant,
    /// 通信のタイプ。
    pub network_type: NetworkType,
    /// トランスポートの種類。
    pub transport: NetworkTransport,
    /// method (例: "workspace.list", "document.open")。
    pub method: String,
    /// request_id (Request/Response の対応付け用)。
    pub request_id: Option<String>,
    /// 結果状態。
    pub status: NetworkStatus,
    /// 処理時間 (Request の場合)。
    pub duration: Option<std::time::Duration>,
}
