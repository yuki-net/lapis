use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
