pub use lapis_client_api::{
    AuthMethod, AuthRequired, AuthenticateRequest, ClientHello, ClientMessage, InvalidSecretHex,
    PairRequest, PairedResponse, ServerMessage,
};
pub use lapis_wire::{REMOTE_WEBSOCKET_PATH, REMOTE_WEBSOCKET_PROTOCOL};

pub(crate) fn protocol_error(code: &str) -> ServerMessage {
    let code = lapis_client_api::ErrorCode::try_new(code)
        .expect("built-in protocol error code must be valid");
    ServerMessage::Error(lapis_client_api::ProtocolError::new(code))
}
