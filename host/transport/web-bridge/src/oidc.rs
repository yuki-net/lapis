//! Web 向け OIDC Authorization Code + PKCE の開始 endpoint。

use std::{
    collections::HashMap,
    env::VarError,
    error::Error,
    fmt,
    sync::Mutex,
    time::{Duration, Instant},
};

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};
use client_core::pkce::{Pkce, authorization_url};

use crate::Bridge;

const DEFAULT_SCOPES: &str = "openid profile";
const DEFAULT_PENDING_TTL: Duration = Duration::from_secs(300);
const MAX_PENDING: usize = 256;

/// Web bridge が IdP へリダイレクトするための設定。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OidcConfig {
    pub authorization_endpoint: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
}

impl OidcConfig {
    pub fn from_env() -> Result<Option<Self>, OidcError> {
        Self::from_lookup(|name| match std::env::var(name) {
            Ok(value) => Ok(Some(value)),
            Err(VarError::NotPresent) => Ok(None),
            Err(VarError::NotUnicode(_)) => Err(OidcError::InvalidEnv(name.to_owned())),
        })
    }

    #[cfg(test)]
    fn from_values(lookup: impl Fn(&str) -> Option<String>) -> Result<Option<Self>, OidcError> {
        Self::from_lookup(|name| Ok(lookup(name)))
    }

    fn from_lookup(
        lookup: impl Fn(&str) -> Result<Option<String>, OidcError>,
    ) -> Result<Option<Self>, OidcError> {
        let authorization_endpoint = lookup("LAPIS_WEB_OIDC_AUTHORIZATION_ENDPOINT")?;
        let client_id = lookup("LAPIS_WEB_OIDC_CLIENT_ID")?;
        let redirect_uri = lookup("LAPIS_WEB_OIDC_REDIRECT_URI")?;
        if authorization_endpoint.is_none() && client_id.is_none() && redirect_uri.is_none() {
            return Ok(None);
        }
        let authorization_endpoint =
            authorization_endpoint.ok_or(OidcError::Missing("authorization endpoint"))?;
        let client_id = client_id.ok_or(OidcError::Missing("client id"))?;
        let redirect_uri = redirect_uri.ok_or(OidcError::Missing("redirect uri"))?;
        if client_id.trim().is_empty() {
            return Err(OidcError::Missing("client id"));
        }
        validate_https_url("authorization endpoint", &authorization_endpoint)?;
        validate_https_url("redirect uri", &redirect_uri)?;
        let scopes = lookup("LAPIS_WEB_OIDC_SCOPES")?
            .unwrap_or_else(|| DEFAULT_SCOPES.to_owned())
            .split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if scopes.is_empty() {
            return Err(OidcError::Missing("scopes"));
        }
        Ok(Some(Self {
            authorization_endpoint,
            client_id,
            redirect_uri,
            scopes,
        }))
    }
}

fn validate_https_url(name: &'static str, value: &str) -> Result<(), OidcError> {
    if value.chars().any(char::is_control) {
        return Err(OidcError::InvalidUrl(name));
    }
    let url = url::Url::parse(value).map_err(|_| OidcError::InvalidUrl(name))?;
    if url.scheme() != "https" {
        return Err(OidcError::InsecureEndpoint);
    }
    if url.host_str().is_none() {
        return Err(OidcError::InvalidUrl(name));
    }
    if url.fragment().is_some() {
        return Err(OidcError::FragmentNotAllowed(name));
    }
    Ok(())
}

/// 進行中の Authorization Code を state で管理する。
pub struct OidcBroker {
    config: OidcConfig,
    pending_ttl: Duration,
    pending: Mutex<HashMap<String, Pending>>,
}

struct Pending {
    /// callback の token 交換で使う。
    #[allow(dead_code)]
    verifier: String,
    expires_at: Instant,
}

impl OidcBroker {
    pub fn new(config: OidcConfig) -> Self {
        Self {
            config,
            pending_ttl: DEFAULT_PENDING_TTL,
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub fn from_env() -> Result<Option<Self>, OidcError> {
        Ok(OidcConfig::from_env()?.map(Self::new))
    }

    /// PKCE の verifier / state を作り、Authorization Endpoint の URL を返す。
    pub fn start(&self) -> Result<String, OidcError> {
        let mut verifier_entropy = [0u8; 32];
        let mut state_entropy = [0u8; 16];
        getrandom::fill(&mut verifier_entropy).map_err(|_| OidcError::Random)?;
        getrandom::fill(&mut state_entropy).map_err(|_| OidcError::Random)?;
        let pkce =
            Pkce::from_entropy(&verifier_entropy, &state_entropy).map_err(|_| OidcError::Pkce)?;
        let mut pending = self.pending.lock().map_err(|_| OidcError::State)?;
        let now = Instant::now();
        pending.retain(|_, value| value.expires_at > now);
        if pending.len() >= MAX_PENDING {
            return Err(OidcError::TooManyPending);
        }
        let url = authorization_url(
            &self.config.authorization_endpoint,
            &self.config.client_id,
            &self.config.redirect_uri,
            &self
                .config
                .scopes
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            &pkce,
        )
        .map_err(|_| OidcError::Pkce)?;
        pending.insert(
            pkce.state().to_owned(),
            Pending {
                verifier: pkce.verifier().to_owned(),
                expires_at: now + self.pending_ttl,
            },
        );
        Ok(url)
    }
}

#[derive(Debug)]
pub enum OidcError {
    Missing(&'static str),
    InsecureEndpoint,
    InvalidEnv(String),
    InvalidUrl(&'static str),
    FragmentNotAllowed(&'static str),
    Random,
    Pkce,
    State,
    TooManyPending,
}

impl fmt::Display for OidcError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(field) => write!(formatter, "OIDC {field} is not configured"),
            Self::InsecureEndpoint => formatter.write_str("OIDC endpoints must use HTTPS"),
            Self::InvalidEnv(name) => {
                write!(formatter, "OIDC environment variable {name} is invalid")
            }
            Self::InvalidUrl(field) => write!(formatter, "OIDC {field} is not a valid URL"),
            Self::FragmentNotAllowed(field) => {
                write!(formatter, "OIDC {field} must not contain a fragment")
            }
            Self::Random => formatter.write_str("OIDC random generation failed"),
            Self::Pkce => formatter.write_str("OIDC PKCE generation failed"),
            Self::State => formatter.write_str("OIDC state is unavailable"),
            Self::TooManyPending => formatter.write_str("OIDC pending login limit reached"),
        }
    }
}

impl Error for OidcError {}

/// `/auth/oidc/start`。OIDC が未設定なら 404 を返す。
pub async fn start(State(bridge): State<Bridge>) -> Response {
    let Some(broker) = bridge.oidc.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match broker.start() {
        Ok(url) => Redirect::temporary(&url).into_response(),
        Err(error) => {
            eprintln!("OIDC login を開始できません: {error}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> OidcConfig {
        OidcConfig {
            authorization_endpoint: "https://idp.example/authorize".to_owned(),
            client_id: "lapis-web".to_owned(),
            redirect_uri: "https://host.example/auth/oidc/callback".to_owned(),
            scopes: vec!["openid".to_owned(), "profile".to_owned()],
        }
    }

    fn lookup(values: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let values = values
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect::<HashMap<_, _>>();
        move |name| values.get(name).cloned()
    }

    #[test]
    fn unset_oidc_is_disabled() {
        assert_eq!(OidcConfig::from_values(lookup(&[])).unwrap(), None);
    }

    #[test]
    fn partial_config_is_rejected() {
        let error = OidcConfig::from_values(lookup(&[("LAPIS_WEB_OIDC_CLIENT_ID", "lapis-web")]))
            .unwrap_err();
        assert!(matches!(error, OidcError::Missing(_)));
    }

    #[test]
    fn http_endpoint_is_rejected() {
        let error = OidcConfig::from_values(lookup(&[
            (
                "LAPIS_WEB_OIDC_AUTHORIZATION_ENDPOINT",
                "http://idp.example/authorize",
            ),
            ("LAPIS_WEB_OIDC_CLIENT_ID", "lapis-web"),
            (
                "LAPIS_WEB_OIDC_REDIRECT_URI",
                "https://host.example/callback",
            ),
        ]))
        .unwrap_err();
        assert!(matches!(error, OidcError::InsecureEndpoint));
    }

    #[test]
    fn empty_client_id_is_rejected() {
        let error = OidcConfig::from_values(lookup(&[
            (
                "LAPIS_WEB_OIDC_AUTHORIZATION_ENDPOINT",
                "https://idp.example/authorize",
            ),
            ("LAPIS_WEB_OIDC_CLIENT_ID", "  "),
            (
                "LAPIS_WEB_OIDC_REDIRECT_URI",
                "https://host.example/callback",
            ),
        ]))
        .unwrap_err();
        assert!(matches!(error, OidcError::Missing("client id")));
    }

    #[test]
    fn fragment_endpoint_is_rejected() {
        let error = OidcConfig::from_values(lookup(&[
            (
                "LAPIS_WEB_OIDC_AUTHORIZATION_ENDPOINT",
                "https://idp.example/authorize#fragment",
            ),
            ("LAPIS_WEB_OIDC_CLIENT_ID", "lapis-web"),
            (
                "LAPIS_WEB_OIDC_REDIRECT_URI",
                "https://host.example/callback",
            ),
        ]))
        .unwrap_err();
        assert!(matches!(error, OidcError::FragmentNotAllowed(_)));
    }

    #[test]
    fn newline_endpoint_is_rejected() {
        let error = OidcConfig::from_values(lookup(&[
            (
                "LAPIS_WEB_OIDC_AUTHORIZATION_ENDPOINT",
                "https://idp.example/authorize\n",
            ),
            ("LAPIS_WEB_OIDC_CLIENT_ID", "lapis-web"),
            (
                "LAPIS_WEB_OIDC_REDIRECT_URI",
                "https://host.example/callback",
            ),
        ]))
        .unwrap_err();
        assert!(matches!(error, OidcError::InvalidUrl(_)));
    }

    #[test]
    fn pending_limit_is_enforced() {
        let broker = OidcBroker::new(config());
        {
            let mut pending = broker.pending.lock().unwrap();
            for index in 0..MAX_PENDING {
                pending.insert(
                    format!("state-{index}"),
                    Pending {
                        verifier: "a".repeat(43),
                        expires_at: Instant::now() + Duration::from_secs(60),
                    },
                );
            }
        }
        assert!(matches!(broker.start(), Err(OidcError::TooManyPending)));
    }

    #[test]
    fn start_builds_authorization_url_and_stores_state() {
        let broker = OidcBroker::new(config());
        let url = broker.start().unwrap();
        assert!(url.starts_with("https://idp.example/authorize?"));
        assert!(url.contains("client_id=lapis-web"));
        assert!(url.contains("scope=openid%20profile"));
        assert!(url.contains("code_challenge_method=S256"));
        let pending = broker.pending.lock().unwrap();
        assert_eq!(pending.len(), 1);
        let (state, value) = pending.iter().next().unwrap();
        assert!(url.contains(&format!("state={state}")));
        assert!(!value.verifier.is_empty());
    }
}
