use std::{error::Error, fmt};

use crate::{
    ClientHello, EventEnvelope, ProtocolError, RequestEnvelope, ResponseEnvelope, ServerHello,
    WorkspaceId,
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::{CredentialId, InvalidCredentialId};

const SECRET_BYTES: usize = 32;

/// Host が提示する認証方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// 接続時の認証方式。
pub enum AuthMethod {
    Token,
    Oidc,
}

/// 接続直後に Host が返す認証方式の一覧。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Host が要求する認証方式の提示。
pub struct AuthRequired {
    pub methods: Vec<AuthMethod>,
    /// ネイティブ Client が OIDC ログインに使う公開設定。
    /// 未設定・旧 Host では省略される。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oidc: Option<OidcClientConfig>,
}

/// OIDC Provider の公開情報。秘密情報は含めない。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// OIDC Device 承認に必要な Client 設定。
pub struct OidcClientConfig {
    pub issuer: String,
    pub client_id: String,
    pub scopes: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
/// remote transport の Client 送信 message。
pub enum ClientMessage {
    #[serde(rename = "hello")]
    Hello(ClientHello),
    #[serde(rename = "auth.pair")]
    Pair(PairRequest),
    #[serde(rename = "auth.authenticate")]
    Authenticate(AuthenticateRequest),
    #[serde(rename = "auth.oidc")]
    Oidc(OidcRequest),
    #[serde(rename = "request")]
    Request(RequestEnvelope),
}

#[derive(Serialize, Deserialize)]
/// ペアリング要求。
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

    pub fn take_token(&mut self) -> Result<[u8; SECRET_BYTES], InvalidSecretHex> {
        self.token.take_bytes()
    }
}

#[derive(Serialize, Deserialize)]
/// credential による認証要求。
pub struct AuthenticateRequest {
    pub hello: ClientHello,
    /// 未指定の場合は Host が最初の Workspace を選ぶ。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<WorkspaceId>,
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
            workspace_id: Some(workspace_id),
            credential_id: credential_id.as_str().to_owned(),
            secret: SecretHex::encode(secret),
        }
    }

    pub fn new_auto(
        hello: ClientHello,
        credential_id: &CredentialId,
        secret: &[u8; SECRET_BYTES],
    ) -> Self {
        Self {
            hello,
            workspace_id: None,
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

/// OIDC Provider が発行した ID Token を Host へ提示する。
#[derive(Serialize, Deserialize)]
/// OIDC Device 承認の要求。
pub struct OidcRequest {
    pub hello: ClientHello,
    /// 未指定の場合は Host が最初の Workspace を選ぶ。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<WorkspaceId>,
    id_token: String,
}

impl OidcRequest {
    pub fn new(hello: ClientHello, workspace_id: WorkspaceId, id_token: String) -> Self {
        Self {
            hello,
            workspace_id: Some(workspace_id),
            id_token,
        }
    }

    pub fn new_auto(hello: ClientHello, id_token: String) -> Self {
        Self {
            hello,
            workspace_id: None,
            id_token,
        }
    }

    pub fn id_token(&self) -> &str {
        &self.id_token
    }
}

impl Drop for OidcRequest {
    fn drop(&mut self) {
        self.id_token.zeroize();
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "payload")]
/// remote transport の Host 送信 message。
pub enum ServerMessage {
    #[serde(rename = "auth.required")]
    AuthRequired(AuthRequired),
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
/// ペアリング結果。
pub struct PairedResponse {
    pub hello: ServerHello,
    credential_id: String,
    secret: SecretHex,
}

impl PairedResponse {
    pub fn new(
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
/// secret が hex として不正。
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
    use crate::{CURRENT_PROTOCOL, CapabilitySet, ClientId, ClientKind, ProtocolRange};

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
