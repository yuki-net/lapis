use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

pub(crate) const SECRET_BYTES: usize = 32;
pub(crate) type SecretDigest = [u8; 32];

pub(crate) fn digest(secret: &[u8; SECRET_BYTES]) -> SecretDigest {
    Sha256::digest(secret).into()
}

pub(crate) fn constant_time_digest_matches(
    secret: &[u8; SECRET_BYTES],
    expected: &SecretDigest,
) -> bool {
    digest(secret).ct_eq(expected).into()
}
