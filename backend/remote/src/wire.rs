use std::{error::Error, fmt};

use lapis_client_api::{
    ClientHello, ErrorCode, EventEnvelope, ProtocolError, RequestEnvelope, ResponseEnvelope,
    ServerHello, WorkspaceId,
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::{CredentialId, InvalidCredentialId};

pub const REMOTE_WEBSOCKET_PATH: &str = "/remote";
pub const REMOTE_WEBSOCKET_PROTOCOL: &str = "lapis.v0";
const SECRET_BYTES: usize = 32;

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum ClientMessage {
    #[serde(rename = "auth.pair")]
    Pair(PairRequest),
    #[serde(rename = "auth.authenticate")]
    Authenticate(AuthenticateRequest),
    #[serde(rename = "request")]
    Request(RequestEnvelope),
}

#[derive(Serialize, Deserialize)]
pub struct PairRequest {
    pub hello: ClientHello,
    token: SecretHex,
}

impl PairRequest {
    pub fn new(hello: ClientHello, token: &[u8; SECRET_BYTES]) -> Self {
        Self {
            hello,
            token: SecretHex::encode(token),
        }
    }

    pub(crate) fn take_token(&mut self) -> Result<[u8; SECRET_BYTES], InvalidSecretHex> {
        self.token.take_bytes()
    }
}

#[derive(Serialize, Deserialize)]
pub struct AuthenticateRequest {
    pub hello: ClientHello,
    pub workspace_id: WorkspaceId,
    credential_id: String,
    secret: SecretHex,
}

impl AuthenticateRequest {
    pub fn new(
        hello: ClientHello,
        workspace_id: WorkspaceId,
        credential_id: &CredentialId,
        secret: &[u8; SECRET_BYTES],
    ) -> Self {
        Self {
            hello,
            workspace_id,
            credential_id: credential_id.as_str().to_owned(),
            secret: SecretHex::encode(secret),
        }
    }

    pub(crate) fn credential_id(&self) -> Result<CredentialId, InvalidCredentialId> {
        CredentialId::parse(self.credential_id.clone())
    }

    pub(crate) fn take_secret(&mut self) -> Result<[u8; SECRET_BYTES], InvalidSecretHex> {
        self.secret.take_bytes()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
pub enum ServerMessage {
    #[serde(rename = "auth.paired")]
    Paired(PairedResponse),
    #[serde(rename = "auth.authenticated")]
    Authenticated(ServerHello),
    #[serde(rename = "response")]
    Response(ResponseEnvelope),
    #[serde(rename = "event")]
    Event(EventEnvelope),
    #[serde(rename = "error")]
    Error(ProtocolError),
}

#[derive(Serialize, Deserialize)]
pub struct PairedResponse {
    pub hello: ServerHello,
    credential_id: String,
    secret: SecretHex,
}

impl PairedResponse {
    pub(crate) fn new(
        hello: ServerHello,
        credential_id: &CredentialId,
        secret: &[u8; SECRET_BYTES],
    ) -> Self {
        Self {
            hello,
            credential_id: credential_id.as_str().to_owned(),
            secret: SecretHex::encode(secret),
        }
    }

    pub fn credential_id(&self) -> Result<CredentialId, InvalidCredentialId> {
        CredentialId::parse(self.credential_id.clone())
    }

    pub fn take_secret(&mut self) -> Result<[u8; SECRET_BYTES], InvalidSecretHex> {
        self.secret.take_bytes()
    }
}

pub(crate) fn protocol_error(code: &str) -> ServerMessage {
    let code = ErrorCode::try_new(code).expect("built-in protocol error code must be valid");
    ServerMessage::Error(ProtocolError::new(code))
}

#[derive(Serialize, Deserialize)]
#[serde(transparent)]
struct SecretHex(String);

impl SecretHex {
    fn encode(bytes: &[u8; SECRET_BYTES]) -> Self {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut encoded = String::with_capacity(SECRET_BYTES * 2);
        for byte in bytes {
            encoded.push(HEX[(byte >> 4) as usize] as char);
            encoded.push(HEX[(byte & 0x0f) as usize] as char);
        }
        Self(encoded)
    }

    fn take_bytes(&mut self) -> Result<[u8; SECRET_BYTES], InvalidSecretHex> {
        if self.0.len() != SECRET_BYTES * 2 || !self.0.is_ascii() {
            return Err(InvalidSecretHex);
        }
        let mut decoded = [0; SECRET_BYTES];
        for (index, pair) in self.0.as_bytes().chunks_exact(2).enumerate() {
            decoded[index] = (decode_nibble(pair[0])? << 4) | decode_nibble(pair[1])?;
        }
        self.0.zeroize();
        Ok(decoded)
    }
}

impl Drop for SecretHex {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

fn decode_nibble(byte: u8) -> Result<u8, InvalidSecretHex> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(InvalidSecretHex),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidSecretHex;

impl fmt::Display for InvalidSecretHex {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("secret must be 64 lowercase hexadecimal characters")
    }
}

impl Error for InvalidSecretHex {}

#[cfg(test)]
mod tests {
    use super::*;
    use lapis_client_api::{CURRENT_PROTOCOL, CapabilitySet, ClientId, ClientKind, ProtocolRange};

    fn hello() -> ClientHello {
        ClientHello {
            protocol: ProtocolRange::exact(CURRENT_PROTOCOL),
            client_id: ClientId::try_new("mobile-1").unwrap(),
            client_name: "Mobile".to_owned(),
            client_kind: ClientKind::Android,
            requested_capabilities: CapabilitySet::default(),
        }
    }

    #[test]
    fn authentication_secret_round_trips_without_debug_exposure() {
        let credential_id = CredentialId::parse("0123456789abcdef0123456789abcdef").unwrap();
        let request = AuthenticateRequest::new(
            hello(),
            WorkspaceId::try_new("workspace-1").unwrap(),
            &credential_id,
            &[0xab; SECRET_BYTES],
        );
        let json = serde_json::to_string(&ClientMessage::Authenticate(request)).unwrap();
        let mut restored = match serde_json::from_str::<ClientMessage>(&json).unwrap() {
            ClientMessage::Authenticate(request) => request,
            _ => panic!("expected authentication request"),
        };

        assert_eq!(restored.take_secret().unwrap(), [0xab; SECRET_BYTES]);
        assert_eq!(restored.secret.0, "");
    }

    #[test]
    fn secret_rejects_non_canonical_hex() {
        let mut uppercase = SecretHex("AB".repeat(SECRET_BYTES));
        let mut short = SecretHex("ab".repeat(SECRET_BYTES - 1));

        assert_eq!(uppercase.take_bytes(), Err(InvalidSecretHex));
        assert_eq!(short.take_bytes(), Err(InvalidSecretHex));
    }
}
