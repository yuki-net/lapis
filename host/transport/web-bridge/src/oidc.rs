//! Web 向け OIDC Authorization Code + PKCE の開始と callback。

use std::{
    collections::HashMap,
    env::VarError,
    error::Error,
    fmt,
    sync::Mutex,
    time::{Duration, Instant},
};

use axum::{
    extract::{Query, State},
    http::{HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
};
use client_core::pkce::{Pkce, authorization_url};
use serde::Deserialize;

use crate::Bridge;

const DEFAULT_SCOPES: &str = "openid profile";
const DEFAULT_PENDING_TTL: Duration = Duration::from_secs(300);
const MAX_PENDING: usize = 256;
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_TOKEN_RESPONSE: usize = 1024 * 1024;

/// Web bridge が IdP へリダイレクトするための設定。
#[derive(Clone, PartialEq, Eq)]
pub struct OidcConfig {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
}

impl fmt::Debug for OidcConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OidcConfig")
            .field("authorization_endpoint", &self.authorization_endpoint)
            .field("token_endpoint", &self.token_endpoint)
            .field("client_id", &self.client_id)
            .field(
                "client_secret",
                &self.client_secret.as_ref().map(|_| "[redacted]"),
            )
            .field("redirect_uri", &self.redirect_uri)
            .field("scopes", &self.scopes)
            .finish()
    }
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
        let token_endpoint = lookup("LAPIS_WEB_OIDC_TOKEN_ENDPOINT")?;
        let client_id = lookup("LAPIS_WEB_OIDC_CLIENT_ID")?;
        let client_secret = lookup("LAPIS_WEB_OIDC_CLIENT_SECRET")?;
        let redirect_uri = lookup("LAPIS_WEB_OIDC_REDIRECT_URI")?;
        if authorization_endpoint.is_none()
            && token_endpoint.is_none()
            && client_id.is_none()
            && client_secret.is_none()
            && redirect_uri.is_none()
        {
            return Ok(None);
        }
        let authorization_endpoint =
            authorization_endpoint.ok_or(OidcError::Missing("authorization endpoint"))?;
        let token_endpoint = token_endpoint.ok_or(OidcError::Missing("token endpoint"))?;
        let client_id = client_id.ok_or(OidcError::Missing("client id"))?;
        let redirect_uri = redirect_uri.ok_or(OidcError::Missing("redirect uri"))?;
        if client_id.trim().is_empty() {
            return Err(OidcError::Missing("client id"));
        }
        if client_secret.as_deref() == Some("") {
            return Err(OidcError::Missing("client secret"));
        }
        validate_https_url("authorization endpoint", &authorization_endpoint)?;
        validate_https_url("token endpoint", &token_endpoint)?;
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
            token_endpoint,
            client_id,
            client_secret,
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

    /// state を一度だけ消費し、対応する PKCE verifier を返す。
    fn take_pending(&self, state: &str) -> Result<String, OidcError> {
        let mut pending = self.pending.lock().map_err(|_| OidcError::State)?;
        let value = pending.remove(state).ok_or(OidcError::UnknownState)?;
        if value.expires_at <= Instant::now() {
            return Err(OidcError::ExpiredState);
        }
        Ok(value.verifier)
    }

    /// Authorization Code を token endpoint で ID Token と交換する。
    async fn exchange(&self, code: &str, verifier: &str) -> Result<String, OidcError> {
        let client = reqwest::Client::builder()
            .timeout(EXCHANGE_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| OidcError::HttpClient)?;
        let form = [
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", self.config.redirect_uri.as_str()),
            ("client_id", self.config.client_id.as_str()),
            ("code_verifier", verifier),
        ];
        let mut request = client.post(&self.config.token_endpoint).form(&form);
        if let Some(secret) = self.config.client_secret.as_deref() {
            request = request.basic_auth(&self.config.client_id, Some(secret));
        }
        let mut response = request.send().await.map_err(|_| OidcError::TokenRequest)?;
        if !response.status().is_success() {
            return Err(OidcError::TokenStatus(response.status().as_u16()));
        }
        let body = read_limited(&mut response).await?;
        let token: TokenResponse =
            serde_json::from_slice(&body).map_err(|_| OidcError::TokenResponse)?;
        if token.error.is_some() {
            return Err(OidcError::TokenRejected);
        }
        token
            .id_token
            .filter(|value| !value.is_empty())
            .ok_or(OidcError::MissingIdToken)
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    id_token: Option<String>,
    error: Option<String>,
}

/// 応答本文を上限付きで読み切る。
async fn read_limited(response: &mut reqwest::Response) -> Result<Vec<u8>, OidcError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_TOKEN_RESPONSE as u64)
    {
        return Err(OidcError::TokenResponseTooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| OidcError::TokenRequest)?
    {
        if body.len() + chunk.len() > MAX_TOKEN_RESPONSE {
            return Err(OidcError::TokenResponseTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
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
    UnknownState,
    ExpiredState,
    HttpClient,
    TokenRequest,
    TokenStatus(u16),
    TokenResponse,
    TokenResponseTooLarge,
    TokenRejected,
    MissingIdToken,
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
            Self::UnknownState => formatter.write_str("OIDC callback state is unknown"),
            Self::ExpiredState => formatter.write_str("OIDC callback state has expired"),
            Self::HttpClient => formatter.write_str("OIDC HTTP client is unavailable"),
            Self::TokenRequest => formatter.write_str("OIDC token request failed"),
            Self::TokenStatus(status) => write!(formatter, "OIDC token endpoint returned {status}"),
            Self::TokenResponse => formatter.write_str("OIDC token response is invalid"),
            Self::TokenResponseTooLarge => {
                formatter.write_str("OIDC token response exceeds the size limit")
            }
            Self::TokenRejected => formatter.write_str("OIDC token endpoint rejected the code"),
            Self::MissingIdToken => formatter.write_str("OIDC token response has no ID token"),
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
        Ok(url) => redirect(&url),
        Err(error) => {
            eprintln!("OIDC login を開始できません: {error}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

fn redirect(url: &str) -> Response {
    let mut response = Redirect::temporary(url).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// `/auth/oidc/callback`。code を ID Token と交換し、opener へ postMessage する。
pub async fn callback(
    State(bridge): State<Bridge>,
    Query(query): Query<CallbackQuery>,
) -> Response {
    let Some(broker) = bridge.oidc.as_ref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(state) = query.state.filter(|value| !value.is_empty()) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let verifier = match broker.take_pending(&state) {
        Ok(verifier) => verifier,
        Err(error) => {
            eprintln!("OIDC callback を拒否しました: {error}");
            return StatusCode::BAD_REQUEST.into_response();
        }
    };
    if query.error.is_some() {
        eprintln!("OIDC callback が IdP エラーを返しました");
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Some(code) = query.code.filter(|value| !value.is_empty()) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    match broker.exchange(&code, &verifier).await {
        Ok(id_token) => completion_page(&bridge.origin, &id_token),
        Err(error) => {
            eprintln!("OIDC token 交換に失敗しました: {error}");
            StatusCode::BAD_GATEWAY.into_response()
        }
    }
}

/// ログイン完了ページ。ID Token は URL やログへ出さず、opener への message だけで渡す。
fn completion_page(origin: &str, id_token: &str) -> Response {
    let id_token = json_for_script(id_token);
    let origin = json_for_script(origin);
    let body = format!(
        r#"<!doctype html>
<html lang="ja">
<head>
<meta charset="utf-8">
<meta name="referrer" content="no-referrer">
<title>Lapis ログイン</title>
</head>
<body>
<p>ログインが完了しました。このウィンドウは自動的に閉じます。</p>
<script>
(() => {{
  const payload = {{ type: "lapis.oidc", idToken: {id_token} }};
  if (window.opener) {{
    window.opener.postMessage(payload, {origin});
    window.close();
  }}
}})();
</script>
</body>
</html>
"#
    );
    let mut response = Html(body).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; script-src 'unsafe-inline'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        ),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

/// JSON 文字列を script 要素へ埋め込める形にする。
fn json_for_script(value: &str) -> String {
    serde_json::to_string(value)
        .unwrap_or_else(|_| "\"\"".to_owned())
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{Router, http::HeaderMap, routing::post};
    use serde_json::json;
    use tokio::task::JoinHandle;

    use super::*;

    fn config() -> OidcConfig {
        OidcConfig {
            authorization_endpoint: "https://idp.example/authorize".to_owned(),
            token_endpoint: "https://idp.example/token".to_owned(),
            client_id: "lapis-web".to_owned(),
            client_secret: None,
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

    fn env_values(token_endpoint: &str) -> Vec<(&'static str, String)> {
        vec![
            (
                "LAPIS_WEB_OIDC_AUTHORIZATION_ENDPOINT",
                "https://idp.example/authorize".to_owned(),
            ),
            ("LAPIS_WEB_OIDC_TOKEN_ENDPOINT", token_endpoint.to_owned()),
            ("LAPIS_WEB_OIDC_CLIENT_ID", "lapis-web".to_owned()),
            (
                "LAPIS_WEB_OIDC_REDIRECT_URI",
                "https://host.example/callback".to_owned(),
            ),
        ]
    }

    fn env_lookup<'a>(
        values: &'a [(&'static str, String)],
    ) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.clone())
        }
    }

    #[derive(Clone)]
    struct MockResponse {
        status: StatusCode,
        body: String,
    }

    type Capture = Arc<Mutex<Option<(HeaderMap, String)>>>;

    async fn token_server(response: MockResponse) -> (String, Capture, JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let captured: Capture = Arc::new(Mutex::new(None));
        let handler_capture = captured.clone();
        let app = Router::new().route(
            "/token",
            post(move |headers: HeaderMap, body: String| {
                let captured = handler_capture.clone();
                let response = response.clone();
                async move {
                    *captured.lock().unwrap() = Some((headers, body));
                    (
                        response.status,
                        [(header::CONTENT_TYPE, "application/json")],
                        response.body,
                    )
                }
            }),
        );
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}/token"), captured, task)
    }

    #[test]
    fn config_debug_redacts_client_secret() {
        let mut config = config();
        config.client_secret = Some("super-secret".to_owned());
        let debug = format!("{config:?}");
        assert!(!debug.contains("super-secret"));
        assert!(debug.contains("[redacted]"));
    }

    #[test]
    fn redirect_is_not_cached() {
        let response = redirect("https://idp.example/authorize?state=x");
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
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
    fn missing_token_endpoint_is_rejected() {
        let error = OidcConfig::from_values(lookup(&[
            (
                "LAPIS_WEB_OIDC_AUTHORIZATION_ENDPOINT",
                "https://idp.example/authorize",
            ),
            ("LAPIS_WEB_OIDC_CLIENT_ID", "lapis-web"),
            (
                "LAPIS_WEB_OIDC_REDIRECT_URI",
                "https://host.example/callback",
            ),
        ]))
        .unwrap_err();
        assert!(matches!(error, OidcError::Missing("token endpoint")));
    }

    #[test]
    fn http_endpoint_is_rejected() {
        let error = OidcConfig::from_values(lookup(&[
            (
                "LAPIS_WEB_OIDC_AUTHORIZATION_ENDPOINT",
                "http://idp.example/authorize",
            ),
            ("LAPIS_WEB_OIDC_TOKEN_ENDPOINT", "https://idp.example/token"),
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
    fn http_token_endpoint_is_rejected() {
        let error = OidcConfig::from_values(lookup(&[
            (
                "LAPIS_WEB_OIDC_AUTHORIZATION_ENDPOINT",
                "https://idp.example/authorize",
            ),
            ("LAPIS_WEB_OIDC_TOKEN_ENDPOINT", "http://idp.example/token"),
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
        let values = vec![
            (
                "LAPIS_WEB_OIDC_AUTHORIZATION_ENDPOINT",
                "https://idp.example/authorize".to_owned(),
            ),
            (
                "LAPIS_WEB_OIDC_TOKEN_ENDPOINT",
                "https://idp.example/token".to_owned(),
            ),
            ("LAPIS_WEB_OIDC_CLIENT_ID", "  ".to_owned()),
            (
                "LAPIS_WEB_OIDC_REDIRECT_URI",
                "https://host.example/callback".to_owned(),
            ),
        ];
        let error = OidcConfig::from_values(env_lookup(&values)).unwrap_err();
        assert!(matches!(error, OidcError::Missing("client id")));
    }

    #[test]
    fn empty_client_secret_is_rejected() {
        let mut values = env_values("https://idp.example/token");
        values.push(("LAPIS_WEB_OIDC_CLIENT_SECRET", String::new()));
        let error = OidcConfig::from_values(env_lookup(&values)).unwrap_err();
        assert!(matches!(error, OidcError::Missing("client secret")));
    }

    #[test]
    fn fragment_endpoint_is_rejected() {
        let error = OidcConfig::from_values(lookup(&[
            (
                "LAPIS_WEB_OIDC_AUTHORIZATION_ENDPOINT",
                "https://idp.example/authorize#fragment",
            ),
            ("LAPIS_WEB_OIDC_TOKEN_ENDPOINT", "https://idp.example/token"),
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
            ("LAPIS_WEB_OIDC_TOKEN_ENDPOINT", "https://idp.example/token"),
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

    #[test]
    fn take_pending_consumes_state_once() {
        let broker = OidcBroker::new(config());
        broker.start().unwrap();
        let state = {
            let pending = broker.pending.lock().unwrap();
            pending.keys().next().unwrap().clone()
        };
        let verifier = broker.take_pending(&state).unwrap();
        assert_eq!(verifier.len(), 43);
        assert!(matches!(
            broker.take_pending(&state),
            Err(OidcError::UnknownState)
        ));
    }

    #[test]
    fn expired_pending_is_rejected_and_removed() {
        let broker = OidcBroker::new(config());
        broker.pending.lock().unwrap().insert(
            "expired".to_owned(),
            Pending {
                verifier: "a".repeat(43),
                expires_at: Instant::now() - Duration::from_secs(1),
            },
        );
        assert!(matches!(
            broker.take_pending("expired"),
            Err(OidcError::ExpiredState)
        ));
        assert!(broker.pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn exchange_posts_pkce_and_returns_id_token() {
        let (endpoint, captured, _task) = token_server(MockResponse {
            status: StatusCode::OK,
            body: json!({ "id_token": "id-token-1" }).to_string(),
        })
        .await;
        let mut config = config();
        config.client_secret = Some("secret-1".to_owned());
        config.token_endpoint = endpoint;
        let broker = OidcBroker::new(config);
        let token = broker.exchange("code-1", "verifier-1").await.unwrap();
        assert_eq!(token, "id-token-1");

        let (headers, body) = captured.lock().unwrap().clone().unwrap();
        assert_eq!(
            headers[header::AUTHORIZATION],
            "Basic bGFwaXMtd2ViOnNlY3JldC0x"
        );
        let fields = url::form_urlencoded::parse(body.as_bytes())
            .into_owned()
            .collect::<HashMap<_, _>>();
        assert_eq!(fields["grant_type"], "authorization_code");
        assert_eq!(fields["code"], "code-1");
        assert_eq!(fields["code_verifier"], "verifier-1");
        assert_eq!(fields["client_id"], "lapis-web");
        assert_eq!(
            fields["redirect_uri"],
            "https://host.example/auth/oidc/callback"
        );
    }

    #[tokio::test]
    async fn public_client_exchange_omits_basic_auth() {
        let (endpoint, captured, _task) = token_server(MockResponse {
            status: StatusCode::OK,
            body: json!({ "id_token": "id-token-1" }).to_string(),
        })
        .await;
        let mut config = config();
        config.token_endpoint = endpoint;
        let broker = OidcBroker::new(config);
        broker.exchange("code-1", "verifier-1").await.unwrap();
        let (headers, _) = captured.lock().unwrap().clone().unwrap();
        assert!(!headers.contains_key(header::AUTHORIZATION));
    }

    #[tokio::test]
    async fn non_success_status_is_rejected() {
        let (endpoint, _, _task) = token_server(MockResponse {
            status: StatusCode::BAD_REQUEST,
            body: json!({ "error": "invalid_grant" }).to_string(),
        })
        .await;
        let mut config = config();
        config.token_endpoint = endpoint;
        let broker = OidcBroker::new(config);
        assert!(matches!(
            broker.exchange("code-1", "verifier-1").await,
            Err(OidcError::TokenStatus(400))
        ));
    }

    #[tokio::test]
    async fn token_error_payload_is_rejected() {
        let (endpoint, _, _task) = token_server(MockResponse {
            status: StatusCode::OK,
            body: json!({ "error": "invalid_grant" }).to_string(),
        })
        .await;
        let mut config = config();
        config.token_endpoint = endpoint;
        let broker = OidcBroker::new(config);
        assert!(matches!(
            broker.exchange("code-1", "verifier-1").await,
            Err(OidcError::TokenRejected)
        ));
    }

    #[tokio::test]
    async fn missing_id_token_is_rejected() {
        let (endpoint, _, _task) = token_server(MockResponse {
            status: StatusCode::OK,
            body: "{}".to_owned(),
        })
        .await;
        let mut config = config();
        config.token_endpoint = endpoint;
        let broker = OidcBroker::new(config);
        assert!(matches!(
            broker.exchange("code-1", "verifier-1").await,
            Err(OidcError::MissingIdToken)
        ));
    }

    #[tokio::test]
    async fn oversized_token_response_is_rejected() {
        let (endpoint, _, _task) = token_server(MockResponse {
            status: StatusCode::OK,
            body: "a".repeat(MAX_TOKEN_RESPONSE + 1),
        })
        .await;
        let mut config = config();
        config.token_endpoint = endpoint;
        let broker = OidcBroker::new(config);
        assert!(matches!(
            broker.exchange("code-1", "verifier-1").await,
            Err(OidcError::TokenResponseTooLarge)
        ));
    }

    #[test]
    fn script_json_escapes_html() {
        assert_eq!(
            json_for_script("a</script>&b"),
            "\"a\\u003c/script\\u003e\\u0026b\""
        );
    }

    #[tokio::test]
    async fn completion_page_posts_token_to_the_bridge_origin() {
        let response = completion_page("http://localhost:5173", "id-token-1");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("idToken: \"id-token-1\""));
        assert!(body.contains("window.opener.postMessage(payload, \"http://localhost:5173\")"));
        assert!(body.contains("window.close()"));
    }
}
