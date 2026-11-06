//! ClientとHostが共用する接続識別子。
pub const REMOTE_WEBSOCKET_PATH: &str = "/remote";
pub const REMOTE_WEBSOCKET_PROTOCOL: &str = "lapis.v0";
pub const REMOTE_QUIC_PROTOCOL: &[u8] = b"lapis.v0";
