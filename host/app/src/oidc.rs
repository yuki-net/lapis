//! Host の OIDC 設定を環境変数から組み立てる。

use std::sync::Arc;

use remote::{OidcConfig, OidcVerifier};

const ISSUER: &str = "LAPIS_OIDC_ISSUER";
const AUDIENCE: &str = "LAPIS_OIDC_AUDIENCE";
const JWKS_FILE: &str = "LAPIS_OIDC_JWKS_FILE";
const JWKS_URI: &str = "LAPIS_OIDC_JWKS_URI";
const CLOCK_SKEW: &str = "LAPIS_OIDC_CLOCK_SKEW_SECONDS";
const DEFAULT_CLOCK_SKEW: u64 = 60;

#[derive(Debug, PartialEq, Eq)]
enum JwksSource {
    File(String),
    Uri(String),
}

#[derive(Debug, PartialEq, Eq)]
struct Settings {
    issuer: String,
    audience: String,
    jwks: JwksSource,
    clock_skew_seconds: u64,
}

/// OIDC が設定されていれば検証器を作る。未設定なら None を返す。
pub fn verifier_from_env() -> Result<Option<Arc<OidcVerifier>>, String> {
    let Some(settings) = settings(|name| std::env::var(name).ok())? else {
        return Ok(None);
    };
    let config = OidcConfig::new_rs256(
        settings.issuer,
        settings.audience,
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
            verifier.start_refresh();
            verifier
        }
    };
    Ok(Some(Arc::new(verifier)))
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
    let jwks = match (jwks_file, jwks_uri) {
        (Some(path), None) => JwksSource::File(path),
        (None, Some(uri)) => JwksSource::Uri(uri),
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
        assert_eq!(settings.jwks, JwksSource::File("jwks.json".to_owned()));
        assert_eq!(settings.clock_skew_seconds, DEFAULT_CLOCK_SKEW);
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
