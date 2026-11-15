//! `.lapis` package の manifest 署名検証。
//!
//! リリース署名（`scripts/release/manifest.py`、`clients/desktop/launcher`）と
//! 同じ規則を使う。Ed25519、`release/trusted-keys.json` の信頼済み公開鍵、
//! `signature` を除いた JSON を key 順に並べて区切りを詰めた UTF-8 bytes への署名。
//! 拡張 package は別の manifest schema だが、鍵と正規化・検証の規則は同じである。

use std::{collections::BTreeMap, fmt};

use base64::Engine as _;
use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::Value;

const REGISTRY_SCHEMA: i64 = 1;
const REGISTRY_FIELDS: [&str; 2] = ["schema", "keys"];

/// launcher と同じ repository の registry を build 時に埋め込む。
///
/// 信頼の起点はこの file だけであり、package に含めた公開鍵を信頼しない。
const REPOSITORY_REGISTRY: &str = include_str!("../../../release/trusted-keys.json");

/// 署名鍵の識別子から検証鍵を引く registry。
#[derive(Clone)]
pub struct TrustedKeys(BTreeMap<String, VerifyingKey>);

impl TrustedKeys {
    /// repository の `release/trusted-keys.json` を使う。Host はこれだけを使う。
    pub fn repository() -> Result<Self, SignatureError> {
        Self::parse(REPOSITORY_REGISTRY.as_bytes())
    }

    /// registry の bytes を明示して作る。検証規則を確かめるテストが使う。
    pub fn parse(raw: &[u8]) -> Result<Self, SignatureError> {
        let value: Value = serde_json::from_slice(raw).map_err(|error| {
            SignatureError::new(format!("公開鍵 registry を読めません: {error}"))
        })?;
        let object = value
            .as_object()
            .ok_or_else(|| SignatureError::new("公開鍵 registry が object でない"))?;
        reject_unknown_and_missing(object, &REGISTRY_FIELDS, "公開鍵 registry")?;
        if object["schema"].as_i64() != Some(REGISTRY_SCHEMA) {
            return Err(SignatureError::new("公開鍵 registry の schema が不正"));
        }
        let keys = object["keys"]
            .as_object()
            .ok_or_else(|| SignatureError::new("公開鍵 registry の keys が object でない"))?;
        let mut parsed = BTreeMap::new();
        for (key_id, encoded) in keys {
            if key_id.is_empty() {
                return Err(SignatureError::new("公開鍵 registry の key_id が空"));
            }
            let encoded = encoded
                .as_str()
                .ok_or_else(|| SignatureError::new(format!("{key_id} の公開鍵が文字列でない")))?;
            let bytes = decode_base64(encoded).map_err(|error| {
                SignatureError::new(format!("{key_id} の公開鍵が base64 でない: {error}"))
            })?;
            let bytes: [u8; 32] = bytes
                .try_into()
                .map_err(|_| SignatureError::new(format!("{key_id} の公開鍵は 32 byte")))?;
            let key = VerifyingKey::from_bytes(&bytes)
                .map_err(|_| SignatureError::new(format!("{key_id} の公開鍵が不正")))?;
            // 恒等元・小位数点を鍵として受けると、s=0 などの署名が任意の message で通る。
            if key.is_weak() {
                return Err(SignatureError::new(format!(
                    "{key_id} の公開鍵が恒等元・小位数点"
                )));
            }
            parsed.insert(key_id.clone(), key);
        }
        Ok(Self(parsed))
    }

    fn get(&self, key_id: &str) -> Option<&VerifyingKey> {
        self.0.get(key_id)
    }
}

/// 署名対象の正規化 bytes。`signature` を除いた manifest を、key 順を sort し
/// 区切りを詰めた JSON（UTF-8）にする。
///
/// `verify_signature` が内部で組み立てるものと同一であり、テストが署名対象を
/// 再現するために公開する。
#[cfg(test)]
pub(crate) fn canonical_manifest_bytes(value: &Value) -> Result<Vec<u8>, SignatureError> {
    canonical(value)
}

/// manifest の署名を検証する。
pub(crate) fn verify_signature(
    keys: &TrustedKeys,
    key_id: &str,
    encoded: &str,
    payload: &Value,
) -> Result<(), SignatureError> {
    let key = keys
        .get(key_id)
        .ok_or_else(|| SignatureError::new(format!("信頼していない key_id: {key_id}")))?;
    let bytes = decode_base64(encoded)
        .map_err(|error| SignatureError::new(format!("署名の base64 が不正: {error}")))?;
    let signature =
        Signature::from_slice(&bytes).map_err(|_| SignatureError::new("署名は 64 byte"))?;
    let message = canonical(payload)?;
    key.verify_strict(&message, &signature)
        .map_err(|_| SignatureError::new("manifest の署名が一致しません"))
}

fn canonical(value: &Value) -> Result<Vec<u8>, SignatureError> {
    let mut output = Vec::new();
    write_canonical(value, &mut output)?;
    Ok(output)
}

fn write_canonical(value: &Value, output: &mut Vec<u8>) -> Result<(), SignatureError> {
    match value {
        Value::Object(object) => {
            let mut keys: Vec<&String> = object.keys().collect();
            keys.sort_unstable();
            output.push(b'{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                output.extend(serde_json::to_vec(key).map_err(|error| {
                    SignatureError::new(format!("署名対象を作れません: {error}"))
                })?);
                output.push(b':');
                write_canonical(&object[key], output)?;
            }
            output.push(b'}');
        }
        Value::Array(items) => {
            output.push(b'[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                write_canonical(item, output)?;
            }
            output.push(b']');
        }
        other => output.extend(
            serde_json::to_vec(other)
                .map_err(|error| SignatureError::new(format!("署名対象を作れません: {error}")))?,
        ),
    }
    Ok(())
}

fn reject_unknown_and_missing(
    object: &serde_json::Map<String, Value>,
    fields: &[&str],
    label: &str,
) -> Result<(), SignatureError> {
    if let Some(unknown) = object.keys().find(|key| !fields.contains(&key.as_str())) {
        return Err(SignatureError::new(format!(
            "{label} に未知の field: {unknown}"
        )));
    }
    if let Some(missing) = fields.iter().find(|field| !object.contains_key(**field)) {
        return Err(SignatureError::new(format!(
            "{label} に field が足りない: {missing}"
        )));
    }
    Ok(())
}

fn decode_base64(encoded: &str) -> Result<Vec<u8>, base64::DecodeError> {
    base64::engine::general_purpose::STANDARD.decode(encoded)
}

/// 公開鍵 registry・署名の不備。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureError(String);

impl SignatureError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for SignatureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for SignatureError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_bytes_match_the_release_normalization() {
        // `scripts/release/manifest.py` の `canonical_bytes`（sort_keys=True,
        // separators=(",", ":")）が同じ入力に対して返す bytes。
        let value = serde_json::json!({
            "schema": 1,
            "kind": "extension",
            "id": "dev.example.terminal",
            "version": "0.2.0",
            "requires": [{"contract": "lapis.extension", "minimum": "1.0.0", "maximum": "1.0.0"}],
            "files": [{"path": "extension.wasm", "sha256": "00"}],
            "key_id": "k",
            "signature": "ignored",
        });
        let mut payload = value.clone();
        payload.as_object_mut().unwrap().remove("signature");
        assert_eq!(
            canonical_manifest_bytes(&payload).unwrap(),
            br#"{"files":[{"path":"extension.wasm","sha256":"00"}],"id":"dev.example.terminal","key_id":"k","kind":"extension","requires":[{"contract":"lapis.extension","maximum":"1.0.0","minimum":"1.0.0"}],"schema":1,"version":"0.2.0"}"#
                .to_vec()
        );
    }

    /// `scripts/release` が署名した配布物の manifest と、同じ payload から Python が
    /// 作った正規化 bytes を使い、拡張 manifest の正規化がリリース署名と同一であることを
    /// 固定する。schema が違っても署名対象の組み立てが食い違えば落ちる。
    #[test]
    fn canonical_bytes_match_the_python_generated_fixture() {
        let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../clients/desktop/launcher/tests/fixtures");
        let raw = std::fs::read(fixtures.join("distribution/manifest.json"))
            .expect("launcher の署名済み manifest");
        let mut value: Value = serde_json::from_slice(&raw).expect("manifest json");
        value.as_object_mut().expect("object").remove("signature");
        let expected = std::fs::read(fixtures.join("canonical-manifest.json"))
            .expect("python が署名した正規化 bytes");
        assert_eq!(canonical_manifest_bytes(&value).unwrap(), expected);
    }

    #[test]
    fn registry_rejects_weak_and_malformed_keys() {
        // 恒等元の encoding。
        let registry =
            br#"{"schema":1,"keys":{"weak":"AQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="}}"#;
        assert!(TrustedKeys::parse(registry).is_err());
        assert!(TrustedKeys::parse(br#"{"schema":1,"keys":{},"extra":true}"#).is_err());
        assert!(TrustedKeys::parse(br#"{"schema":2,"keys":{}}"#).is_err());
        assert!(TrustedKeys::parse(br#"{"schema":1,"keys":{"k":"not base64"}}"#).is_err());
        assert!(TrustedKeys::parse(br#"{"schema":1,"keys":{"k":"AA=="}}"#).is_err());
    }

    #[test]
    fn repository_registry_is_usable() {
        let keys = TrustedKeys::repository().unwrap();
        assert!(keys.get("lapis-release").is_some());
    }
}
