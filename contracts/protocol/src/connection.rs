use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Client と Host の接続段階。
pub enum ConnectionState {
    #[default]
    Disconnected,
    Discovering,
    Connecting,
    Pairing,
    Authenticating,
    Negotiating,
    Synchronizing,
    Connected,
    Reconnecting,
    Rejected,
}
