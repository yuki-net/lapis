//! OIDC ID Token の検証。Host は Resource Server として JWKS の公開鍵だけを使う。

use std::{error::Error, fmt};

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::Deserialize;

/// Host が信頼する OIDC Provider の設定。
#[derive(Clone, Debug)]
pub struct OidcConfig {
    pub issuer: String,
    pub audience: String,
    pub allowed_algorithms: Vec<Algorithm>,
    pub clock_skew_seconds: u64,
}

impl OidcConfig {
    pub fn new(
        issuer: impl Into<String>,
        audience: impl Into<String>,
        allowed_algorithms: Vec<Algorithm>,
        clock_skew_seconds: u64,
    ) -> Result<Self, OidcError> {
        let config = Self {
            issuer: issuer.into(),
            audience: audience.into(),
            allowed_algorithms,
            clock_skew_seconds,
        };
        if config.issuer.is_empty() {
            return Err(OidcError::InvalidConfig("issuer is empty"));
        }
        if config.audience.is_empty() {
            return Err(OidcError::InvalidConfig("audience is empty"));
        }
        if config.allowed_algorithms.is_empty() {
            return Err(OidcError::InvalidConfig("allowed algorithms are empty"));
        }
        if config.allowed_algorithms.iter().any(|algorithm| {
            matches!(
                algorithm,
                Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512
            )
        }) {
            return Err(OidcError::InvalidConfig(
                "symmetric algorithms are not allowed",
            ));
        }
        Ok(config)
    }
}

/// 検証済みの ID Token Claim。Host はこの値だけを Session の所有者判定に使う。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OidcClaims {
    pub subject: String,
    pub issuer: String,
    pub audience: Vec<String>,
    pub expires_at: u64,
    pub client_id: Option<String>,
}

/// JWKS を保持し、ID Token を検証する。
#[derive(Debug)]
pub struct OidcVerifier {
    config: OidcConfig,
    jwks: JwkSet,
}

impl OidcVerifier {
    pub fn new(config: OidcConfig, jwks: JwkSet) -> Result<Self, OidcError> {
        if jwks.keys.is_empty() {
            return Err(OidcError::InvalidConfig("JWKS is empty"));
        }
        Ok(Self { config, jwks })
    }

    pub fn from_jwks_json(config: OidcConfig, jwks_json: &str) -> Result<Self, OidcError> {
        let jwks = serde_json::from_str(jwks_json).map_err(|_| OidcError::InvalidJwks)?;
        Self::new(config, jwks)
    }

    pub fn config(&self) -> &OidcConfig {
        &self.config
    }

    /// 署名・issuer・audience・exp・nbf を検証し、Session に使う Claim を返す。
    pub fn verify(&self, token: &str) -> Result<OidcClaims, OidcError> {
        let header = decode_header(token).map_err(|_| OidcError::MalformedToken)?;
        if !self.config.allowed_algorithms.contains(&header.alg) {
            return Err(OidcError::UnsupportedAlgorithm);
        }
        let key = self.decoding_key(header.kid.as_deref())?;
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[&self.config.issuer]);
        validation.set_audience(&[&self.config.audience]);
        validation.leeway = self.config.clock_skew_seconds;
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        let data =
            decode::<Claims>(token, &key, &validation).map_err(|_| OidcError::InvalidClaims)?;
        let audience = match data.claims.aud {
            Audience::One(value) => vec![value],
            Audience::Many(values) => values,
        };
        Ok(OidcClaims {
            subject: data.claims.sub,
            issuer: data.claims.iss,
            audience,
            expires_at: data.claims.exp,
            client_id: data.claims.azp.or(data.claims.client_id),
        })
    }

    fn decoding_key(&self, kid: Option<&str>) -> Result<DecodingKey, OidcError> {
        let jwk = match kid {
            Some(kid) => self.jwks.find(kid).ok_or(OidcError::UnknownKey)?,
            None if self.jwks.keys.len() == 1 => &self.jwks.keys[0],
            None => return Err(OidcError::UnknownKey),
        };
        DecodingKey::from_jwk(jwk).map_err(|_| OidcError::InvalidKey)
    }
}

#[derive(Debug)]
pub enum OidcError {
    InvalidConfig(&'static str),
    InvalidJwks,
    InvalidKey,
    UnknownKey,
    MalformedToken,
    UnsupportedAlgorithm,
    InvalidClaims,
}

impl fmt::Display for OidcError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidConfig(reason) => reason,
            Self::InvalidJwks => "JWKS is invalid",
            Self::InvalidKey => "JWK is not a usable verification key",
            Self::UnknownKey => "token key is not trusted",
            Self::MalformedToken => "token header is invalid",
            Self::UnsupportedAlgorithm => "token algorithm is not allowed",
            Self::InvalidClaims => "token claims are invalid",
        })
    }
}

impl Error for OidcError {}

#[derive(Deserialize)]
struct Claims {
    sub: String,
    iss: String,
    aud: Audience,
    exp: u64,
    #[serde(default)]
    azp: Option<String>,
    #[serde(default)]
    client_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode, get_current_timestamp};
    use serde_json::json;

    const TEST_PEM: &str = r#"-----BEGIN PRIVATE KEY-----
MIIEvAIBADANBgkqhkiG9w0BAQEFAASCBKYwggSiAgEAAoIBAQC75fnniddTEX6z
yKfXLoMROp6pYDTACkXExf9Xy5EkSRc4Lc9k2eGYAcEXISw0v7IoXiG400C3WBrj
Bztvki9qacq0SrCSCqGk+vMJ8wZpNPsSK4y1ruZgXQCQ61NjbXv0gsoVIMLAUL0+
XGnavTizTK7E8YPF06W34Jt1rqHBNPW9wwM1CpWKmfd5F0rflGLu3bVTvzB6kxzQ
8cDU9XjQBz9TDroLzMrmNxGikhsoX0PVZv30RsbgEsSDiLW5GOIfsKh00P6IHZI1
ysFRsrh/4trFVq4rPGC2CV9AgTX897QeBWkdduv/IMfJCCV6R1QlwEcKVYqq6kXA
Ir6yrM9jAgMBAAECggEAST+/muwo9XiqAJM2HxRCQxqrlJLtLXoztaCMSOESXWdi
9mMhaQrpP5VbfVVQwuWCIeuEUOWF4pOls6BzoqyF8IF6ZKuDRwN+/W5ZBaVod+YQ
BIbWrVMX+zP2Twh2MxoQywKbnf3t/gPbqm5jU8MVUgyQSzVtrday0UptwERKSaNb
aIjKj2XDOQu6FOT2BNfiJwbiCxLTSrOWW4/aSi4H9NN3xDnhRAzsk3Q0tp3uPjwN
FdMUPY1XdixdNqRjKY+Ll0+XWKzukMhLQtjCvalx4yVwQpOlkbmTBc0BMjQeJIxr
U5K25MGPoTl74HDV/zGxp2CP6FGgWicS1ZL4Bgf8ZQKBgQD9dA0QPU8w2eu81dD+
eY1VVkEq+TiKUfgDFKfFkdyCOXekiQ+fM1S3/cxHv+8z6+DZ1NVYsbvJBV2gf6eg
q2zpw5Ltb73HCfi+ncgrB2+jGYLzw0ZsaUqpUSYpV5RjPs7MQ6TzpivRof2cx1Xe
tIJBAVDJqf+cSOs8ZPdaCgI6nwKBgQC9yUzoJig8b6Pm6cQXpPXLHkj4vHG7u1PF
fVH5W6I3zkYXQKOCvDr28+63o+8K926S8iIcSwd4zng+iyATb4gTkOU+zqmumvAs
2NC0snCUYmscYVnjdtTn7EeWaXNUkTvZnAVcZVZzs1/t4SD1sbKRrmF/rqC5lXNT
W8hfflN4vQKBgG83e3wCyzsuhSlJ0fm4sPIfdV08GzlnGqOPbsddaBukyAFlO3uq
dKSANCHP8VEhUnuDz1KqOzLMrfb9zTrNvm8dqN2TyP0MPSC379wh2TUCvtnkeISd
92sw9sPxcGXUa75qeHmKE3Rb1wPy3JVrNDT+NMspU8bqoH7R91CFT9EVAoGAVlWd
g7nf8eZDZEcRakQjgDwNrAFlp6YG7L8erFXQDJzsa6V+/HkORcsf3DBaDN4UUgqO
E9rZ0k3yi1kUxWKmP2hZsOGodQKMoYocHydRaV8AJM/waTZkkgB9bGIaEe1jkZGV
Krv6wPVnfTo9mSc1+WmfdssE2IexB8mn9DDuXe0CgYBPwQsVwtC0bixAZmVv83V+
n6MTTfmQLH8PsvFGsAZ16xBIKfEzzheE0lFQFod00k1OzUj+IYJRmp671qPpcTT7
t0f+7q2VROR7iJ6VMePTAuTKUgXM5Q6lmHm3ocrbft7gI9egFt/giCtC7VgFHTdo
QpNdCy1VwA1iKz1CyJaSeQ==
-----END PRIVATE KEY-----"#;

    const TEST_JWKS: &str = r#"{"keys":[{"kty":"RSA","kid":"test-key","use":"sig","alg":"RS256","n":"u-X554nXUxF-s8in1y6DETqeqWA0wApFxMX_V8uRJEkXOC3PZNnhmAHBFyEsNL-yKF4huNNAt1ga4wc7b5IvamnKtEqwkgqhpPrzCfMGaTT7EiuMta7mYF0AkOtTY2179ILKFSDCwFC9Plxp2r04s0yuxPGDxdOlt-Cbda6hwTT1vcMDNQqVipn3eRdK35Ri7t21U78wepMc0PHA1PV40Ac_Uw66C8zK5jcRopIbKF9D1Wb99EbG4BLEg4i1uRjiH7CodND-iB2SNcrBUbK4f-LaxVauKzxgtglfQIE1_Pe0HgVpHXbr_yDHyQglekdUJcBHClWKqupFwCK-sqzPYw","e":"AQAB"}]}"#;

    fn config() -> OidcConfig {
        OidcConfig::new("https://idp.example", "lapis", vec![Algorithm::RS256], 60).unwrap()
    }

    fn verifier() -> OidcVerifier {
        OidcVerifier::from_jwks_json(config(), TEST_JWKS).unwrap()
    }

    fn token(claims: serde_json::Value) -> String {
        let header = Header {
            alg: Algorithm::RS256,
            kid: Some("test-key".to_owned()),
            ..Default::default()
        };
        let key = EncodingKey::from_rsa_pem(TEST_PEM.as_bytes()).unwrap();
        encode(&header, &claims, &key).unwrap()
    }

    #[test]
    fn valid_token_returns_claims() {
        let now = get_current_timestamp();
        let claims = json!({
            "sub": "alice",
            "iss": "https://idp.example",
            "aud": "lapis",
            "exp": now + 300,
            "nbf": now - 1,
            "azp": "lapis-web",
        });
        let verified = verifier().verify(&token(claims)).unwrap();
        assert_eq!(verified.subject, "alice");
        assert_eq!(verified.audience, vec!["lapis".to_owned()]);
        assert_eq!(verified.client_id.as_deref(), Some("lapis-web"));
    }

    #[test]
    fn wrong_issuer_is_rejected() {
        let now = get_current_timestamp();
        let claims = json!({
            "sub": "alice",
            "iss": "https://other.example",
            "aud": "lapis",
            "exp": now + 300,
        });
        assert!(verifier().verify(&token(claims)).is_err());
    }

    #[test]
    fn wrong_audience_is_rejected() {
        let now = get_current_timestamp();
        let claims = json!({
            "sub": "alice",
            "iss": "https://idp.example",
            "aud": "other",
            "exp": now + 300,
        });
        assert!(verifier().verify(&token(claims)).is_err());
    }

    #[test]
    fn expired_token_is_rejected() {
        let now = get_current_timestamp();
        let claims = json!({
            "sub": "alice",
            "iss": "https://idp.example",
            "aud": "lapis",
            "exp": now - 120,
        });
        assert!(verifier().verify(&token(claims)).is_err());
    }

    #[test]
    fn unknown_key_is_rejected() {
        let now = get_current_timestamp();
        let claims = json!({
            "sub": "alice",
            "iss": "https://idp.example",
            "aud": "lapis",
            "exp": now + 300,
        });
        let header = Header {
            alg: Algorithm::RS256,
            kid: Some("unknown".to_owned()),
            ..Default::default()
        };
        let key = EncodingKey::from_rsa_pem(TEST_PEM.as_bytes()).unwrap();
        let token = encode(&header, &claims, &key).unwrap();
        assert!(verifier().verify(&token).is_err());
    }

    #[test]
    fn symmetric_algorithm_is_rejected() {
        let now = get_current_timestamp();
        let claims = json!({
            "sub": "alice",
            "iss": "https://idp.example",
            "aud": "lapis",
            "exp": now + 300,
        });
        let header = Header::new(Algorithm::HS256);
        let token = encode(&header, &claims, &EncodingKey::from_secret(b"secret")).unwrap();
        assert!(verifier().verify(&token).is_err());
    }
}
