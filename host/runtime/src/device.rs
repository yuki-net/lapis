//! 永続化する Host device の正本表現。

use serde::{Deserialize, Serialize};

/// 認証済み Client の承認状態。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceStatus {
    /// Host 所有者の承認待ち。
    Pending,
    /// 接続を許可する。
    Approved,
    /// 接続を拒否する。
    Revoked,
}

/// Host に接続した Client 端末。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredDevice {
    pub id: String,
    pub client_id: String,
    pub name: String,
    pub kind: String,
    pub status: DeviceStatus,
    pub created_at: u64,
    pub last_seen_at: u64,
    pub approved_at: Option<u64>,
    pub revoked_at: Option<u64>,
}
