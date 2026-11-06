pub use ::wire::{REMOTE_WEBSOCKET_PATH, REMOTE_WEBSOCKET_PROTOCOL};
pub use client_api::{
    AuthMethod, AuthRequired, AuthenticateRequest, ClientHello, ClientMessage, InvalidSecretHex,
    PairRequest, PairedResponse, ServerMessage,
};

pub(crate) fn protocol_error(code: &str) -> ServerMessage {
    let code =
        client_api::ErrorCode::try_new(code).expect("built-in protocol error code must be valid");
    ServerMessage::Error(client_api::ProtocolError::new(code))
}
