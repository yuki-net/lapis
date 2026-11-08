//! OIDC ID Token の検証。Host は Resource Server として JWKS の公開鍵だけを使う。

use std::{
    error::Error,
    fmt,
    io::Read,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::Deserialize;

const JWKS_REFRESH_INTERVAL: Duration = Duration::from_secs(300);
const JWKS_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_JWKS_BYTES: usize = 1024 * 1024;

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

    /// RS256 だけを許可する既定設定。Host の環境変数から組み立てる用途を想定する。
    pub fn new_rs256(
        issuer: impl Into<String>,
        audience: impl Into<String>,
        clock_skew_seconds: u64,
    ) -> Result<Self, OidcError> {
        Self::new(issuer, audience, vec![Algorithm::RS256], clock_skew_seconds)
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
    keys: Arc<RwLock<JwkSet>>,
    jwks_uri: Option<String>,
    refresh_interval: Duration,
    shutdown: Arc<AtomicBool>,
}

impl OidcVerifier {
    pub fn new(config: OidcConfig, jwks: JwkSet) -> Result<Self, OidcError> {
        if jwks.keys.is_empty() {
            return Err(OidcError::InvalidConfig("JWKS is empty"));
        }
        Ok(Self {
            config,
            keys: Arc::new(RwLock::new(jwks)),
            jwks_uri: None,
            refresh_interval: JWKS_REFRESH_INTERVAL,
            shutdown: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn from_jwks_json(config: OidcConfig, jwks_json: &str) -> Result<Self, OidcError> {
        let jwks = serde_json::from_str(jwks_json).map_err(|_| OidcError::InvalidJwks)?;
        Self::new(config, jwks)
    }

    /// HTTPS の JWKS URI から初回取得する。失敗した場合は Host を起動しない。
    pub fn from_remote(config: OidcConfig, jwks_uri: impl Into<String>) -> Result<Self, OidcError> {
        Self::from_remote_inner(config, jwks_uri.into(), false)
    }

    #[cfg(test)]
    pub(crate) fn from_remote_http(
        config: OidcConfig,
        jwks_uri: impl Into<String>,
    ) -> Result<Self, OidcError> {
        Self::from_remote_inner(config, jwks_uri.into(), true)
    }

    fn from_remote_inner(
        config: OidcConfig,
        jwks_uri: String,
        allow_http: bool,
    ) -> Result<Self, OidcError> {
        if !allow_http && !jwks_uri.starts_with("https://") {
            return Err(OidcError::InsecureUri);
        }
        let jwks = fetch_jwks(&jwks_uri)?;
        Ok(Self {
            config,
            keys: Arc::new(RwLock::new(jwks)),
            jwks_uri: Some(jwks_uri),
            refresh_interval: JWKS_REFRESH_INTERVAL,
            shutdown: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn config(&self) -> &OidcConfig {
        &self.config
    }

    /// JWKS を再取得する。失敗時は既存の鍵を保持する。
    pub fn refresh(&self) -> Result<(), OidcError> {
        let Some(uri) = self.jwks_uri.as_ref() else {
            return Ok(());
        };
        let next = fetch_jwks(uri)?;
        *self.keys.write().map_err(|_| OidcError::InvalidJwks)? = next;
        Ok(())
    }

    /// 定期的に JWKS を更新する background thread を開始する。
    pub fn start_refresh(&self) -> Result<(), OidcError> {
        let Some(uri) = self.jwks_uri.clone() else {
            return Ok(());
        };
        let keys = self.keys.clone();
        let shutdown = self.shutdown.clone();
        let interval = self.refresh_interval;
        std::thread::Builder::new()
            .name("lapis-oidc-jwks".to_owned())
            .spawn(move || {
                while !shutdown.load(Ordering::Acquire) {
                    let mut waited = Duration::ZERO;
                    while waited < interval && !shutdown.load(Ordering::Acquire) {
                        std::thread::sleep(Duration::from_secs(1));
                        waited += Duration::from_secs(1);
                    }
                    if shutdown.load(Ordering::Acquire) {
                        break;
                    }
                    match fetch_jwks(&uri) {
                        Ok(next) => {
                            if let Ok(mut keys) = keys.write() {
                                *keys = next;
                            }
                        }
                        Err(error) => {
                            eprintln!("OIDC JWKS の更新に失敗しました: {error}");
                        }
                    }
                }
            })
            .map(|_| ())
            .map_err(|_| OidcError::RefreshUnavailable)
    }

    /// 署名・issuer・audience・exp・nbf を検証し、Session に使う Claim を返す。
    pub fn verify(&self, token: &str) -> Result<OidcClaims, OidcError> {
        match self.verify_once(token) {
            Err(OidcError::UnknownKey) if self.jwks_uri.is_some() => {
                self.refresh()?;
                self.verify_once(token)
            }
            result => result,
        }
    }

    fn verify_once(&self, token: &str) -> Result<OidcClaims, OidcError> {
        let header = decode_header(token).map_err(|_| OidcError::MalformedToken)?;
        if !self.config.allowed_algorithms.contains(&header.alg) {
            return Err(OidcError::UnsupportedAlgorithm);
        }
        let keys = self.keys.read().map_err(|_| OidcError::InvalidJwks)?;
        let key = self.decoding_key(header.kid.as_deref(), &keys)?;
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

    fn decoding_key(&self, kid: Option<&str>, keys: &JwkSet) -> Result<DecodingKey, OidcError> {
        let jwk = match kid {
            Some(kid) => keys.find(kid).ok_or(OidcError::UnknownKey)?,
            None if keys.keys.len() == 1 => &keys.keys[0],
            None => return Err(OidcError::UnknownKey),
        };
        DecodingKey::from_jwk(jwk).map_err(|_| OidcError::InvalidKey)
    }
}

impl Drop for OidcVerifier {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
    }
}

fn fetch_jwks(uri: &str) -> Result<JwkSet, OidcError> {
    let redirects = reqwest::redirect::Policy::custom(|attempt| {
        if attempt.url().scheme() != "https" {
            attempt.error("HTTPS redirect required")
        } else {
            attempt.follow()
        }
    });
    let client = reqwest::blocking::Client::builder()
        .timeout(JWKS_TIMEOUT)
        .redirect(redirects)
        .build()
        .map_err(|_| OidcError::RemoteUnavailable)?;
    let response = client
        .get(uri)
        .send()
        .map_err(|_| OidcError::RemoteUnavailable)?;
    if !response.status().is_success() {
        return Err(OidcError::RemoteUnavailable);
    }
    let mut body = Vec::new();
    response
        .take((MAX_JWKS_BYTES + 1) as u64)
        .read_to_end(&mut body)
        .map_err(|_| OidcError::RemoteUnavailable)?;
    if body.len() > MAX_JWKS_BYTES {
        return Err(OidcError::JwksTooLarge);
    }
    serde_json::from_slice(&body).map_err(|_| OidcError::InvalidJwks)
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
    RemoteUnavailable,
    InsecureUri,
    JwksTooLarge,
    RefreshUnavailable,
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
            Self::RemoteUnavailable => "JWKS is unavailable",
            Self::InsecureUri => "JWKS URI must use HTTPS",
            Self::JwksTooLarge => "JWKS response is too large",
            Self::RefreshUnavailable => "JWKS refresh thread is unavailable",
        })
    }
}

impl Error for OidcError {}

#[cfg(test)]
pub(crate) use tests::{TEST_JWKS, test_token, test_token_for, test_verifier};

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

    pub(crate) const TEST_JWKS: &str = r#"{"keys":[{"kty":"RSA","kid":"test-key","use":"sig","alg":"RS256","n":"u-X554nXUxF-s8in1y6DETqeqWA0wApFxMX_V8uRJEkXOC3PZNnhmAHBFyEsNL-yKF4huNNAt1ga4wc7b5IvamnKtEqwkgqhpPrzCfMGaTT7EiuMta7mYF0AkOtTY2179ILKFSDCwFC9Plxp2r04s0yuxPGDxdOlt-Cbda6hwTT1vcMDNQqVipn3eRdK35Ri7t21U78wepMc0PHA1PV40Ac_Uw66C8zK5jcRopIbKF9D1Wb99EbG4BLEg4i1uRjiH7CodND-iB2SNcrBUbK4f-LaxVauKzxgtglfQIE1_Pe0HgVpHXbr_yDHyQglekdUJcBHClWKqupFwCK-sqzPYw","e":"AQAB"}]}"#;

    fn config() -> OidcConfig {
        OidcConfig::new("https://idp.example", "lapis", vec![Algorithm::RS256], 60).unwrap()
    }

    fn verifier() -> OidcVerifier {
        OidcVerifier::from_jwks_json(config(), TEST_JWKS).unwrap()
    }

    pub(crate) fn test_verifier() -> OidcVerifier {
        verifier()
    }

    pub(crate) fn test_token(subject: &str, expires_at: u64) -> String {
        test_token_for("https://idp.example", subject, expires_at)
    }

    pub(crate) fn test_token_for(issuer: &str, subject: &str, expires_at: u64) -> String {
        let claims = json!({
            "sub": subject,
            "iss": issuer,
            "aud": "lapis",
            "exp": expires_at,
        });
        token(claims)
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
    fn remote_jwks_is_fetched() {
        use std::{
            io::{Read, Write},
            net::TcpListener,
        };

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                TEST_JWKS.len(),
                TEST_JWKS
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        let verifier =
            OidcVerifier::from_remote_http(config(), format!("http://{address}/jwks")).unwrap();
        server.join().unwrap();

        let now = get_current_timestamp();
        let claims = json!({
            "sub": "alice",
            "iss": "https://idp.example",
            "aud": "lapis",
            "exp": now + 300,
        });
        assert!(verifier.verify(&token(claims)).is_ok());
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
