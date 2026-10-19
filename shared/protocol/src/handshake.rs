use serde::{Deserialize, Serialize};

use crate::{CapabilitySet, ClientId, ProtocolRange, ProtocolVersion, SessionId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    Android,
    Ios,
    Desktop,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientHello {
    pub protocol: ProtocolRange,
    pub client_id: ClientId,
    pub client_name: String,
    pub client_kind: ClientKind,
    pub requested_capabilities: CapabilitySet,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerHello {
    pub protocol: ProtocolVersion,
    pub session_id: SessionId,
    pub granted_capabilities: CapabilitySet,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CURRENT_PROTOCOL, CapabilityId};

    #[test]
    fn hello_round_trip_preserves_unknown_capabilities() {
        let hello = ClientHello {
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
