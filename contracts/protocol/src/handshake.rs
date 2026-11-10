use serde::{Deserialize, Serialize};

use crate::{CapabilitySet, ClientId, ProtocolRange, ProtocolVersion, SessionId, WorkspaceId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// 接続元 Client の種類。
pub enum ClientKind {
    Android,
    Ios,
    Desktop,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Client が接続時に提示する protocol・capability。
pub struct HelloRequest {
    pub protocol: ProtocolRange,
    pub client_id: ClientId,
    pub client_name: String,
    pub client_kind: ClientKind,
    pub requested_capabilities: CapabilitySet,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Host が選択した protocol・session・capability。
pub struct HelloResponse {
    pub protocol: ProtocolVersion,
    pub session_id: SessionId,
    pub granted_capabilities: CapabilitySet,
    /// OIDC で認証した場合の検証済み `sub`。Token 方式では省略する。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// Host の永続的な同一性。state directory を持たない Host では省略する。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_id: Option<String>,
    /// Host が選択した Workspace。Client が Workspace を指定しなかった場合に使う。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<WorkspaceId>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CURRENT_PROTOCOL, CapabilityId};

    #[test]
    fn hello_round_trip_preserves_unknown_capabilities() {
        let hello = HelloRequest {
            protocol: ProtocolRange::exact(CURRENT_PROTOCOL),
            client_id: ClientId::try_new("mobile-1").unwrap(),
            client_name: "iPhone".to_owned(),
            client_kind: ClientKind::Ios,
            requested_capabilities: CapabilitySet::try_new([CapabilityId::try_new(
                "future.capability",
            )
            .unwrap()])
            .unwrap(),
        };

        let json = serde_json::to_string(&hello).unwrap();
        let restored = serde_json::from_str(&json).unwrap();

        assert_eq!(hello, restored);
    }
}
