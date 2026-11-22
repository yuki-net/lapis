use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

/// Host に接続する一回限りの招待を発行する。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostInviteRequest {}

/// 認証情報はデバッグ出力へ含めない。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostInviteResponse {
    pub token: String,
}

/// Host が認識している端末を列挙する。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostDeviceListRequest {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostDevice {
    pub id: String,
    pub client_id: String,
    pub name: String,
    pub kind: String,
    pub status: String,
    pub last_seen_at: u64,
    #[serde(default)]
    pub is_current: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostDeviceListResponse {
    pub devices: Vec<HostDevice>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostDeviceRevokeRequest {
    pub device_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostDeviceRevokeResponse {
    pub revoked: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostWebStatusRequest {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostWebStartRequest {
    pub address: String,
    pub port: u16,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostWebStopRequest {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostWebStatus {
    pub url: Option<String>,
    pub addresses: Vec<String>,
    #[serde(default)]
    pub error: Option<String>,
}

impl std::fmt::Debug for HostInviteResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostInviteResponse")
            .field("token", &"[REDACTED]")
            .finish()
    }
}

impl Drop for HostInviteResponse {
    fn drop(&mut self) {
        self.token.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_token_is_not_in_debug_output() {
        let response = HostInviteResponse {
            token: "secret-token".into(),
        };
        assert!(!format!("{response:?}").contains("secret-token"));
        let encoded = serde_json::to_string(&response).unwrap();
        assert_eq!(
            serde_json::from_str::<HostInviteResponse>(&encoded).unwrap(),
            response
        );
    }
}
