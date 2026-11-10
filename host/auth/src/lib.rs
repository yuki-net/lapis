//! Host が永続化する認証・端末の正本表現。

use serde::{Deserialize, Serialize};

/// digest だけを保持し、secret そのものは保存しない。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredCredential {
    pub id: String,
    pub client_id: String,
    /// Workspace 限定 credential の固定先。切り替え可能な credential は持たない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    pub capabilities: Vec<String>,
    pub digest: String,
    pub expires_at: u64,
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_status_round_trips_as_snake_case() {
        let json = serde_json::to_string(&DeviceStatus::Pending).unwrap();
        assert_eq!(json, r#""pending""#);
        assert_eq!(
            serde_json::from_str::<DeviceStatus>(&json).unwrap(),
            DeviceStatus::Pending
        );
    }

    #[test]
    fn stored_credential_keeps_only_the_digest() {
        let credential = StoredCredential {
            id: "credential-1".to_owned(),
            client_id: "client-1".to_owned(),
            workspace_id: Some("workspace-1".to_owned()),
            capabilities: vec!["workspace.list".to_owned()],
            digest: "digest".to_owned(),
            expires_at: 42,
        };
        let json = serde_json::to_string(&credential).unwrap();
        assert!(json.contains("digest"));
        assert_eq!(
            serde_json::from_str::<StoredCredential>(&json).unwrap(),
            credential
        );
    }
}
