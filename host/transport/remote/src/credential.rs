use std::fmt;

use client_api::{ClientId, WorkspaceId};
use zeroize::Zeroize;

use crate::{random::RandomSource, secret::SECRET_BYTES};

pub use client_api::{CredentialId, InvalidCredentialId};

pub(crate) fn generate_id(
    random: &mut impl RandomSource,
) -> Result<CredentialId, super::RandomError> {
    let mut bytes = [0; 16];
    random.fill_bytes(&mut bytes)?;
    Ok(CredentialId::parse(hex_encode(&bytes)).expect("generated credential ID is valid"))
}

/// Clientが保持するcredential。secretはDebug/Serializeされず、サーバーもdigestだけを保存する。
pub struct CredentialHandle {
    id: CredentialId,
    secret: [u8; SECRET_BYTES],
}

impl CredentialHandle {
    pub(crate) fn new(id: CredentialId, secret: [u8; SECRET_BYTES]) -> Self {
        Self { id, secret }
    }

    /// 認証messageを検証済みの固定長byte列へdecodeしたtransport用の生成口。
    pub fn from_parts(id: CredentialId, secret: &mut [u8; SECRET_BYTES]) -> Self {
        Self::new(id, std::mem::take(secret))
    }

    pub fn id(&self) -> &CredentialId {
        &self.id
    }

    /// Transport adapterが認証要求へ詰めるための明示的な取得口。
    pub fn secret_bytes(&self) -> &[u8; SECRET_BYTES] {
        &self.secret
    }
}

impl Drop for CredentialHandle {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}

impl fmt::Debug for CredentialHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialHandle")
            .field("id", &self.id)
            .field("secret", &"REDACTED")
            .finish()
    }
}

pub(crate) struct CredentialRecord {
    pub(crate) client_id: ClientId,
    pub(crate) workspace_id: WorkspaceId,
    pub(crate) capabilities: client_api::CapabilitySet,
    pub(crate) digest: [u8; SECRET_BYTES],
    pub(crate) expires_at: u64,
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_id_parser_requires_the_canonical_wire_form() {
        assert!(CredentialId::parse("0123456789abcdef0123456789abcdef").is_ok());
        assert!(CredentialId::parse("short").is_err());
        assert!(CredentialId::parse("0123456789ABCDEF0123456789ABCDEF").is_err());
    }

    #[test]
    fn credential_handle_takes_and_clears_the_transport_buffer() {
        let id = CredentialId::parse("0123456789abcdef0123456789abcdef").unwrap();
        let mut secret = [7; SECRET_BYTES];
        let credential = CredentialHandle::from_parts(id, &mut secret);

        assert_eq!(secret, [0; SECRET_BYTES]);
        assert_eq!(credential.secret_bytes(), &[7; SECRET_BYTES]);
    }
}
