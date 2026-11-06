//! Host の OIDC 設定を環境変数から組み立てる。

use std::sync::Arc;

use remote::{OidcConfig, OidcVerifier};

const ISSUER: &str = "LAPIS_OIDC_ISSUER";
const AUDIENCE: &str = "LAPIS_OIDC_AUDIENCE";
const JWKS_FILE: &str = "LAPIS_OIDC_JWKS_FILE";
const CLOCK_SKEW: &str = "LAPIS_OIDC_CLOCK_SKEW_SECONDS";
const DEFAULT_CLOCK_SKEW: u64 = 60;

#[derive(Debug, PartialEq, Eq)]
struct Settings {
    issuer: String,
    audience: String,
    jwks_file: String,
    clock_skew_seconds: u64,
}

/// OIDC が設定されていれば検証器を作る。未設定なら None を返す。
pub fn verifier_from_env() -> Result<Option<Arc<OidcVerifier>>, String> {
    let Some(settings) = settings(|name| std::env::var(name).ok())? else {
        return Ok(None);
    };
    let jwks = std::fs::read_to_string(&settings.jwks_file)
        .map_err(|error| format!("JWKSを読めません: {error}"))?;
    let config = OidcConfig::new_rs256(
        settings.issuer,
        settings.audience,
        settings.clock_skew_seconds,
    )
    .map_err(|error| error.to_string())?;
    let verifier =
        OidcVerifier::from_jwks_json(config, &jwks).map_err(|error| error.to_string())?;
    Ok(Some(Arc::new(verifier)))
}

fn settings(lookup: impl Fn(&str) -> Option<String>) -> Result<Option<Settings>, String> {
    let issuer = lookup(ISSUER);
    let audience = lookup(AUDIENCE);
    let jwks_file = lookup(JWKS_FILE);
    let configured = [issuer.as_ref(), audience.as_ref(), jwks_file.as_ref()]
        .into_iter()
        .filter(|value| value.is_some())
        .count();
    if configured == 0 {
        return Ok(None);
    }
    if configured != 3 {
        return Err(format!(
            "{ISSUER}, {AUDIENCE}, {JWKS_FILE} は同時に指定してください"
        ));
    }
    let clock_skew_seconds = lookup(CLOCK_SKEW)
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| format!("{CLOCK_SKEW} が不正です"))
        })
        .transpose()?
        .unwrap_or(DEFAULT_CLOCK_SKEW);
    Ok(Some(Settings {
        issuer: issuer.expect("configured"),
        audience: audience.expect("configured"),
        jwks_file: jwks_file.expect("configured"),
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
        assert!(error.contains("同時に指定"));
    }

    #[test]
    fn full_oidc_uses_default_clock_skew() {
        let settings = settings(lookup(&[
            (ISSUER, "https://idp.example"),
            (AUDIENCE, "lapis"),
            (JWKS_FILE, "jwks.json"),
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(settings.issuer, "https://idp.example");
        assert_eq!(settings.audience, "lapis");
        assert_eq!(settings.jwks_file, "jwks.json");
        assert_eq!(settings.clock_skew_seconds, DEFAULT_CLOCK_SKEW);
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
