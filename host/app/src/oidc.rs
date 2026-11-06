//! Host の OIDC 設定を環境変数から組み立てる。

use std::sync::Arc;

use client_api::OidcClientConfig;
use remote::{OidcConfig, OidcVerifier};

const ISSUER: &str = "LAPIS_OIDC_ISSUER";
const AUDIENCE: &str = "LAPIS_OIDC_AUDIENCE";
const CLIENT_ID: &str = "LAPIS_OIDC_CLIENT_ID";
const SCOPES: &str = "LAPIS_OIDC_SCOPES";
const JWKS_FILE: &str = "LAPIS_OIDC_JWKS_FILE";
const JWKS_URI: &str = "LAPIS_OIDC_JWKS_URI";
const CLOCK_SKEW: &str = "LAPIS_OIDC_CLOCK_SKEW_SECONDS";
const DEFAULT_CLOCK_SKEW: u64 = 60;
const DEFAULT_SCOPES: &str = "openid profile";

/// Host が検証に使う設定と、ネイティブ Client へ提示する公開設定。
pub struct HostOidc {
    pub verifier: Arc<OidcVerifier>,
    pub client: OidcClientConfig,
}

#[derive(Debug, PartialEq, Eq)]
enum JwksSource {
    File(String),
    Uri(String),
}

#[derive(Debug, PartialEq, Eq)]
struct Settings {
    issuer: String,
    audience: String,
    client_id: String,
    scopes: Vec<String>,
    jwks: JwksSource,
    clock_skew_seconds: u64,
}

/// OIDC が設定されていれば検証器とクライアント設定を作る。未設定なら None を返す。
pub fn from_env() -> Result<Option<HostOidc>, String> {
    let Some(settings) = settings(|name| std::env::var(name).ok())? else {
        return Ok(None);
    };
    let config = OidcConfig::new_rs256(
        settings.issuer.clone(),
        settings.audience.clone(),
        settings.clock_skew_seconds,
    )
    .map_err(|error| error.to_string())?;
    let verifier = match settings.jwks {
        JwksSource::File(path) => {
            let jwks = std::fs::read_to_string(&path)
                .map_err(|error| format!("JWKSを読めません: {error}"))?;
            OidcVerifier::from_jwks_json(config, &jwks).map_err(|error| error.to_string())?
        }
        JwksSource::Uri(uri) => {
            let verifier =
                OidcVerifier::from_remote(config, uri).map_err(|error| error.to_string())?;
            verifier
                .start_refresh()
                .map_err(|error| error.to_string())?;
            verifier
        }
    };
    let client = OidcClientConfig {
        issuer: settings.issuer,
        client_id: settings.client_id,
        scopes: settings.scopes,
    };
    Ok(Some(HostOidc {
        verifier: Arc::new(verifier),
        client,
    }))
}

fn settings(lookup: impl Fn(&str) -> Option<String>) -> Result<Option<Settings>, String> {
    let issuer = lookup(ISSUER);
    let audience = lookup(AUDIENCE);
    let jwks_file = lookup(JWKS_FILE);
    let jwks_uri = lookup(JWKS_URI);
    if issuer.is_none() && audience.is_none() && jwks_file.is_none() && jwks_uri.is_none() {
        return Ok(None);
    }
    let (Some(issuer), Some(audience)) = (issuer, audience) else {
        return Err(format!("{ISSUER} と {AUDIENCE} は必須です"));
    };
    if !issuer.starts_with("https://") || issuer.len() == "https://".len() {
        return Err(format!("{ISSUER} は https:// で指定してください"));
    }
    let client_id = lookup(CLIENT_ID).unwrap_or_else(|| audience.clone());
    if client_id.trim().is_empty() {
        return Err(format!("{CLIENT_ID} が空です"));
    }
    let scopes = lookup(SCOPES)
        .unwrap_or_else(|| DEFAULT_SCOPES.to_owned())
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if scopes.is_empty() {
        return Err(format!("{SCOPES} が空です"));
    }
    let jwks = match (jwks_file, jwks_uri) {
        (Some(path), None) => JwksSource::File(path),
        (None, Some(uri)) => {
            if !uri.starts_with("https://") {
                return Err(format!("{JWKS_URI} は https:// で指定してください"));
            }
            JwksSource::Uri(uri)
        }
        (Some(_), Some(_)) => {
            return Err(format!(
                "{JWKS_FILE} と {JWKS_URI} はどちらか一方だけ指定してください"
            ));
        }
        (None, None) => {
            return Err(format!("{JWKS_FILE} または {JWKS_URI} が必要です"));
        }
    };
    let clock_skew_seconds = lookup(CLOCK_SKEW)
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| format!("{CLOCK_SKEW} が不正です"))
        })
        .transpose()?
        .unwrap_or(DEFAULT_CLOCK_SKEW);
    Ok(Some(Settings {
        issuer,
        audience,
        client_id,
        scopes,
        jwks,
        clock_skew_seconds,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(values: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let values = values
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect::<std::collections::HashMap<_, _>>();
        move |name| values.get(name).cloned()
    }

    #[test]
    fn unset_oidc_is_disabled() {
        assert_eq!(settings(lookup(&[])).unwrap(), None);
    }

    #[test]
    fn partial_oidc_is_rejected() {
        let error = settings(lookup(&[(ISSUER, "https://idp.example")])).unwrap_err();
        assert!(error.contains("必須"));
    }

    #[test]
    fn both_jwks_sources_are_rejected() {
        let error = settings(lookup(&[
            (ISSUER, "https://idp.example"),
            (AUDIENCE, "lapis"),
            (JWKS_FILE, "jwks.json"),
            (JWKS_URI, "https://idp.example/jwks"),
        ]))
        .unwrap_err();
        assert!(error.contains("どちらか一方"));
    }

    #[test]
    fn file_oidc_uses_default_clock_skew() {
        let settings = settings(lookup(&[
            (ISSUER, "https://idp.example"),
            (AUDIENCE, "lapis"),
            (JWKS_FILE, "jwks.json"),
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(settings.issuer, "https://idp.example");
        assert_eq!(settings.audience, "lapis");
        assert_eq!(settings.client_id, "lapis");
        assert_eq!(
            settings.scopes,
            vec!["openid".to_owned(), "profile".to_owned()]
        );
        assert_eq!(settings.jwks, JwksSource::File("jwks.json".to_owned()));
        assert_eq!(settings.clock_skew_seconds, DEFAULT_CLOCK_SKEW);
    }

    #[test]
    fn explicit_client_and_scopes_are_used() {
        let settings = settings(lookup(&[
            (ISSUER, "https://idp.example"),
            (AUDIENCE, "lapis"),
            (CLIENT_ID, "lapis-desktop"),
            (SCOPES, "openid email"),
            (JWKS_FILE, "jwks.json"),
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(settings.client_id, "lapis-desktop");
        assert_eq!(
            settings.scopes,
            vec!["openid".to_owned(), "email".to_owned()]
        );
    }

    #[test]
    fn http_issuer_is_rejected() {
        let error = settings(lookup(&[
            (ISSUER, "http://idp.example"),
            (AUDIENCE, "lapis"),
            (JWKS_FILE, "jwks.json"),
        ]))
        .unwrap_err();
        assert!(error.contains("https://"));
    }

    #[test]
    fn empty_scopes_are_rejected() {
        let error = settings(lookup(&[
            (ISSUER, "https://idp.example"),
            (AUDIENCE, "lapis"),
            (SCOPES, "   "),
            (JWKS_FILE, "jwks.json"),
        ]))
        .unwrap_err();
        assert!(error.contains(SCOPES));
    }

    #[test]
    fn uri_oidc_is_accepted() {
        let settings = settings(lookup(&[
            (ISSUER, "https://idp.example"),
            (AUDIENCE, "lapis"),
            (JWKS_URI, "https://idp.example/jwks"),
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(
            settings.jwks,
            JwksSource::Uri("https://idp.example/jwks".to_owned())
        );
    }

    #[test]
    fn http_jwks_uri_is_rejected() {
        let error = settings(lookup(&[
            (ISSUER, "https://idp.example"),
            (AUDIENCE, "lapis"),
            (JWKS_URI, "http://idp.example/jwks"),
        ]))
        .unwrap_err();
        assert!(error.contains("https://"));
    }

    #[test]
    fn invalid_clock_skew_is_rejected() {
        let error = settings(lookup(&[
            (ISSUER, "https://idp.example"),
            (AUDIENCE, "lapis"),
            (JWKS_FILE, "jwks.json"),
            (CLOCK_SKEW, "soon"),
        ]))
        .unwrap_err();
        assert!(error.contains(CLOCK_SKEW));
    }
}
