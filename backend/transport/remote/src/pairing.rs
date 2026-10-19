use std::fmt;

use crate::{random::RandomSource, secret::SECRET_BYTES};
use zeroize::Zeroize;

/// 一度だけ交換できるpairing secret。Debug・Serialize・Displayは実装しない。
pub struct PairingToken([u8; SECRET_BYTES]);

impl PairingToken {
    pub(crate) fn generate(random: &mut impl RandomSource) -> Result<Self, super::RandomError> {
        let mut secret = [0; SECRET_BYTES];
        random.fill_bytes(&mut secret)?;
        Ok(Self(std::mem::take(&mut secret)))
    }

    /// Transport adapterがQRや手入力用の表現へ変換するための明示的な取得口。
    pub fn secret_bytes(&self) -> &[u8; SECRET_BYTES] {
        &self.0
    }

    /// QRまたは手動入力を固定長byte列へdecodeしたtransport用の生成口。
    pub fn from_secret_bytes(secret: &mut [u8; SECRET_BYTES]) -> Self {
        Self(std::mem::take(secret))
    }
}

impl fmt::Debug for PairingToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PairingToken(REDACTED)")
    }
}

impl Drop for PairingToken {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

pub(crate) fn secret_from_token(token: &PairingToken) -> &[u8; SECRET_BYTES] {
    &token.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_token_takes_and_clears_the_transport_buffer() {
        let mut secret = [9; SECRET_BYTES];
        let token = PairingToken::from_secret_bytes(&mut secret);

        assert_eq!(secret, [0; SECRET_BYTES]);
        assert_eq!(token.secret_bytes(), &[9; SECRET_BYTES]);
    }
}
