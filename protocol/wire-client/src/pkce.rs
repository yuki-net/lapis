//! OIDC Authorization Code + PKCE のクライアント側プリミティブ。
//!
//! Token 交換は Host の broker が行う。ここでは verifier / challenge / state の生成と、
//! Authorization Endpoint の URL 組み立てだけを扱う。

use std::{error::Error, fmt};

use sha2::{Digest, Sha256};

/// RFC 7636 の code verifier / challenge と CSRF 用 state。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pkce {
    verifier: String,
    challenge: String,
    state: String,
}

impl Pkce {
    /// 乱数から verifier と state を作る。verifier は 32〜96 byte の entropy を想定する。
    pub fn from_entropy(verifier_entropy: &[u8], state_entropy: &[u8]) -> Result<Self, PkceError> {
        Self::from_verifier(base64url(verifier_entropy), base64url(state_entropy))
    }

    /// 既存の verifier から challenge を導出する。テストと保存済み verifier の再利用に使う。
    pub fn from_verifier(
        verifier: impl Into<String>,
        state: impl Into<String>,
    ) -> Result<Self, PkceError> {
        let verifier = verifier.into();
        if !(43..=128).contains(&verifier.len())
            || !verifier.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
            })
        {
            return Err(PkceError::InvalidVerifier);
        }
        let state = state.into();
        if state.is_empty() {
            return Err(PkceError::EmptyField("state"));
        }
        let challenge = base64url(&Sha256::digest(verifier.as_bytes()));
        Ok(Self {
            verifier,
            challenge,
            state,
        })
    }

    pub fn verifier(&self) -> &str {
        &self.verifier
    }

    pub fn challenge(&self) -> &str {
        &self.challenge
    }

    pub fn state(&self) -> &str {
        &self.state
    }
}

/// Authorization Endpoint の URL を組み立てる。
pub fn authorization_url(
    issuer: &str,
    client_id: &str,
    redirect_uri: &str,
    scopes: &[&str],
    pkce: &Pkce,
) -> Result<String, PkceError> {
    if issuer.is_empty() {
        return Err(PkceError::EmptyField("issuer"));
    }
    if client_id.is_empty() {
        return Err(PkceError::EmptyField("client_id"));
    }
    if redirect_uri.is_empty() {
        return Err(PkceError::EmptyField("redirect_uri"));
    }
    let mut url = issuer.trim_end_matches(['?', '&']).to_owned();
    url.push(if url.contains('?') { '&' } else { '?' });
    let query = [
        ("response_type", "code".to_owned()),
        ("client_id", client_id.to_owned()),
        ("redirect_uri", redirect_uri.to_owned()),
        ("scope", scopes.join(" ")),
        ("state", pkce.state.clone()),
        ("code_challenge", pkce.challenge.clone()),
        ("code_challenge_method", "S256".to_owned()),
    ];
    for (index, (name, value)) in query.iter().enumerate() {
        if index > 0 {
            url.push('&');
        }
        url.push_str(name);
        url.push('=');
        url.push_str(&percent_encode(value));
    }
    Ok(url)
}

#[derive(Debug, PartialEq, Eq)]
pub enum PkceError {
    InvalidVerifier,
    EmptyField(&'static str),
}

impl fmt::Display for PkceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidVerifier => formatter.write_str("code verifier is invalid"),
            Self::EmptyField(field) => write!(formatter, "{field} is empty"),
        }
    }
}

impl Error for PkceError {}

fn base64url(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut output = String::new();
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        output.push(TABLE[(first >> 2) as usize] as char);
        output.push(TABLE[(((first & 0x03) << 4) | (second >> 4)) as usize] as char);
        if chunk.len() > 1 {
            output.push(TABLE[(((second & 0x0f) << 2) | (third >> 6)) as usize] as char);
        }
        if chunk.len() > 2 {
            output.push(TABLE[(third & 0x3f) as usize] as char);
        }
    }
    output
}

fn percent_encode(value: &str) -> String {
    let mut output = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            output.push(byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc7636_vector_is_stable() {
        let pkce =
            Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk", "state-1").unwrap();
        assert_eq!(
            pkce.challenge(),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn entropy_produces_a_valid_verifier() {
        let pkce = Pkce::from_entropy(&[0xab; 32], &[0xcd; 16]).unwrap();
        assert_eq!(pkce.verifier().len(), 43);
        assert_eq!(pkce.state().len(), 22);
        assert!(pkce.challenge().len() >= 43);
    }

    #[test]
    fn authorization_url_encodes_parameters() {
        let pkce =
            Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk", "state 1").unwrap();
        let url = authorization_url(
            "https://idp.example/authorize",
            "lapis-web",
            "https://host.example/auth/callback",
            &["openid", "profile"],
            &pkce,
        )
        .unwrap();
        assert!(url.starts_with("https://idp.example/authorize?response_type=code&"));
        assert!(url.contains("client_id=lapis-web"));
        assert!(url.contains("redirect_uri=https%3A%2F%2Fhost.example%2Fauth%2Fcallback"));
        assert!(url.contains("scope=openid%20profile"));
        assert!(url.contains("state=state%201"));
        assert!(url.contains("code_challenge=E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"));
        assert!(url.contains("code_challenge_method=S256"));
    }

    #[test]
    fn invalid_verifier_is_rejected() {
        assert_eq!(
            Pkce::from_verifier("short", "state").unwrap_err(),
            PkceError::InvalidVerifier
        );
        assert_eq!(
            Pkce::from_verifier("a".repeat(43).replace('a', "!"), "state").unwrap_err(),
            PkceError::InvalidVerifier
        );
    }

    #[test]
    fn empty_state_is_rejected() {
        assert_eq!(
            Pkce::from_verifier("a".repeat(43), "").unwrap_err(),
            PkceError::EmptyField("state")
        );
    }
}
