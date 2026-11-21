//! 署名付き manifest の検証。
//!
//! schema v1 と検証規則は `scripts/release/manifest.py` と同じものを使う。信頼の起点は
//! repository の `release/trusted-keys.json` だけで、launcher には build 時にその内容を
//! 埋め込む。未知・欠落 field はどちらも拒否する。

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{Error, Result};

pub(crate) const MANIFEST_NAME: &str = "manifest.json";

const SCHEMA: i64 = 1;
const MANIFEST_FIELDS: [&str; 8] = [
    "schema",
    "version",
    "breaking",
    "notes_url",
    "generated_at",
    "key_id",
    "assets",
    "signature",
];
const ASSET_FIELDS: [&str; 2] = ["source", "sha256"];
const REGISTRY_FIELDS: [&str; 2] = ["schema", "keys"];

/// launcher 自身に埋め込む信頼済み公開鍵 registry。鍵の追加・失効は launcher の再配布を伴う。
const REPOSITORY_REGISTRY: &str = include_str!("../../../../release/trusted-keys.json");

pub(crate) struct Asset {
    pub(crate) source: String,
    pub(crate) sha256: String,
}

pub(crate) struct Manifest {
    pub(crate) version: String,
    pub(crate) breaking: bool,
    pub(crate) notes_url: String,
    pub(crate) assets: BTreeMap<String, Asset>,
}

/// 署名と SHA-256 を検証済みの配布物。manifest と package が同じ directory にある。
pub(crate) struct Distribution {
    pub(crate) manifest: Manifest,
    pub(crate) packages: BTreeMap<String, PathBuf>,
}

pub(crate) struct TrustedKeys(BTreeMap<String, VerifyingKey>);

impl Clone for TrustedKeys {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl TrustedKeys {
    /// repository の registry を返す。
    pub(crate) fn repository() -> Result<Self> {
        Self::parse(REPOSITORY_REGISTRY.as_bytes())
    }

    pub(crate) fn parse(raw: &[u8]) -> Result<Self> {
        let value: Value = serde_json::from_slice(raw)
            .map_err(|error| Error::new(format!("公開鍵 registry を読めません: {error}")))?;
        let object = value
            .as_object()
            .ok_or_else(|| Error::new("公開鍵 registry が object でない"))?;
        reject_unknown_and_missing(object, &REGISTRY_FIELDS, "公開鍵 registry")?;
        if object["schema"].as_i64() != Some(SCHEMA) {
            return Err(Error::new("公開鍵 registry の schema が不正"));
        }
        let keys = object["keys"]
            .as_object()
            .ok_or_else(|| Error::new("公開鍵 registry の keys が object でない"))?;
        let mut parsed = BTreeMap::new();
        for (key_id, encoded) in keys {
            if key_id.is_empty() {
                return Err(Error::new("公開鍵 registry の key_id が空"));
            }
            let encoded = encoded
                .as_str()
                .ok_or_else(|| Error::new(format!("{key_id} の公開鍵が文字列でない")))?;
            let bytes = decode_base64(encoded).map_err(|error| {
                Error::new(format!("{key_id} の公開鍵が base64 でない: {error}"))
            })?;
            let bytes: [u8; 32] = bytes
                .try_into()
                .map_err(|_| Error::new(format!("{key_id} の公開鍵は 32 byte")))?;
            let key = VerifyingKey::from_bytes(&bytes)
                .map_err(|_| Error::new(format!("{key_id} の公開鍵が不正")))?;
            // 恒等元・小位数点を鍵として受けると、s=0 などの署名が任意の message で通る。
            if key.is_weak() {
                return Err(Error::new(format!("{key_id} の公開鍵が恒等元・小位数点")));
            }
            parsed.insert(key_id.clone(), key);
        }
        Ok(Self(parsed))
    }

    fn get(&self, key_id: &str) -> Option<&VerifyingKey> {
        self.0.get(key_id)
    }
}

/// manifest を読み、署名と全 package の SHA-256 を検証する。
pub(crate) fn verify_distribution(directory: &Path, keys: &TrustedKeys) -> Result<Distribution> {
    let path = directory.join(MANIFEST_NAME);
    let raw = std::fs::read(&path).map_err(|error| {
        Error::new(format!(
            "manifest を読めません ({}): {error}",
            path.display()
        ))
    })?;
    let manifest = parse(&raw)?;
    verify_signature(&raw, keys)?;
    let mut packages = BTreeMap::new();
    for (platform, asset) in &manifest.assets {
        let package = resolve_source(asset, directory);
        if !package.is_file() {
            return Err(Error::new(format!(
                "package がありません: {}",
                package.display()
            )));
        }
        if sha256_file(&package)? != asset.sha256 {
            return Err(Error::new(format!(
                "package の SHA-256 が一致しません: {}",
                package.display()
            )));
        }
        packages.insert(platform.clone(), package);
    }
    Ok(Distribution { manifest, packages })
}

/// 形式だけを検証して読む。署名は `verify_signature` で確認する。
pub(crate) fn parse(raw: &[u8]) -> Result<Manifest> {
    let text = std::str::from_utf8(raw)
        .map_err(|error| Error::new(format!("manifest が UTF-8 でない: {error}")))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let value: Value = serde_json::from_str(text)
        .map_err(|error| Error::new(format!("manifest が JSON でない: {error}")))?;
    let object = value
        .as_object()
        .ok_or_else(|| Error::new("manifest が object でない"))?;
    reject_unknown_and_missing(object, &MANIFEST_FIELDS, "manifest")?;
    if object["schema"].as_i64() != Some(SCHEMA) {
        return Err(Error::new("未対応の schema"));
    }
    let breaking = object["breaking"]
        .as_bool()
        .ok_or_else(|| Error::new("breaking が bool でない"))?;
    let notes_url = object["notes_url"]
        .as_str()
        .ok_or_else(|| Error::new("notes_url が URL でない"))?;
    if !valid_notes_url(notes_url) {
        return Err(Error::new(format!("notes_url が URL でない: {notes_url}")));
    }
    if object["key_id"]
        .as_str()
        .is_none_or(|value| value.is_empty())
    {
        return Err(Error::new("key_id が空"));
    }
    if object["generated_at"].as_str().is_none_or(str::is_empty) {
        return Err(Error::new("generated_at が空"));
    }
    if object["signature"].as_str().is_none() {
        return Err(Error::new("signature が文字列でない"));
    }
    let version = object["version"]
        .as_str()
        .ok_or_else(|| Error::new("version が文字列でない"))?;
    validate_version(version)?;
    Ok(Manifest {
        version: version.to_owned(),
        breaking,
        notes_url: notes_url.to_owned(),
        assets: parse_assets(&object["assets"])?,
    })
}

fn parse_assets(value: &Value) -> Result<BTreeMap<String, Asset>> {
    let object = value
        .as_object()
        .filter(|object| !object.is_empty())
        .ok_or_else(|| Error::new("assets が空"))?;
    let mut assets = BTreeMap::new();
    for (platform, asset) in object {
        if !valid_platform(platform) {
            return Err(Error::new(format!("platform の形式が不正: {platform}")));
        }
        let asset = asset
            .as_object()
            .ok_or_else(|| Error::new(format!("{platform} の asset が object でない")))?;
        reject_unknown_and_missing(asset, &ASSET_FIELDS, platform)?;
        let source = asset["source"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Error::new(format!("{platform} の source が空")))?;
        validate_source(source)?;
        let sha256 = asset["sha256"]
            .as_str()
            .filter(|value| valid_sha256(value))
            .ok_or_else(|| Error::new(format!("{platform} の sha256 が不正")))?;
        assets.insert(
            platform.clone(),
            Asset {
                source: source.to_owned(),
                sha256: sha256.to_owned(),
            },
        );
    }
    Ok(assets)
}

/// 署名を検証する。署名対象は `signature` を除いた manifest の正規化 bytes。
pub(crate) fn verify_signature(raw: &[u8], keys: &TrustedKeys) -> Result<()> {
    let text = std::str::from_utf8(raw)
        .map_err(|error| Error::new(format!("manifest が UTF-8 でない: {error}")))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let value: Value = serde_json::from_str(text)
        .map_err(|error| Error::new(format!("manifest が JSON でない: {error}")))?;
    let object = value
        .as_object()
        .ok_or_else(|| Error::new("manifest が object でない"))?;
    let key_id = object["key_id"]
        .as_str()
        .ok_or_else(|| Error::new("key_id が文字列でない"))?;
    let key = keys
        .get(key_id)
        .ok_or_else(|| Error::new(format!("信頼していない key_id: {key_id}")))?;
    let encoded = object["signature"]
        .as_str()
        .ok_or_else(|| Error::new("signature が文字列でない"))?;
    let signature = decode_base64(encoded)
        .map_err(|error| Error::new(format!("署名の base64 が不正: {error}")))?;
    let signature = Signature::from_slice(&signature).map_err(|_| Error::new("署名は 64 byte"))?;
    let mut payload = object.clone();
    payload.remove("signature");
    let message = canonical(&Value::Object(payload))?;
    key.verify_strict(&message, &signature)
        .map_err(|_| Error::new("manifest の署名が一致しません"))
}

/// 署名対象の正規化 bytes。`scripts/release/manifest.py` の `canonical_bytes` と同じく、
/// key を sort して区切りを詰めた JSON を UTF-8 で返す。
///
/// `serde_json` の map は feature によって key 順が変わるため、ここで明示的に sort する。
pub(crate) fn canonical(value: &Value) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    write_canonical(value, &mut output)?;
    Ok(output)
}

fn write_canonical(value: &Value, output: &mut Vec<u8>) -> Result<()> {
    match value {
        Value::Object(object) => {
            let mut keys: Vec<&String> = object.keys().collect();
            keys.sort_unstable();
            output.push(b'{');
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                output.extend(serde_json::to_vec(key)?);
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
        other => output.extend(serde_json::to_vec(other)?),
    }
    Ok(())
}

fn resolve_source(asset: &Asset, base: &Path) -> PathBuf {
    base.join(&asset.source)
}

pub(crate) fn validate_version(version: &str) -> Result<()> {
    if version.contains("..") || !valid_version(version) {
        return Err(Error::new(format!("version の形式が不正: {version}")));
    }
    Ok(())
}

/// manifest schema v1 が許す version の形式。`scripts/release/manifest.py` の
/// `VERSION_PATTERN` と同じ形だけを受け付ける。
fn valid_version(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    if index == 0 {
        return false;
    }
    while index + 1 < bytes.len() && bytes[index] == b'.' && bytes[index + 1].is_ascii_digit() {
        index += 1;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
    }
    while index < bytes.len() {
        if bytes[index] != b'.' && bytes[index] != b'-' {
            return false;
        }
        index += 1;
        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_alphanumeric() {
            index += 1;
        }
        if index == start {
            return false;
        }
    }
    true
}

fn valid_platform(platform: &str) -> bool {
    !platform.is_empty()
        && platform.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
        && platform.split(['.', '_', '-']).all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

fn valid_sha256(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_notes_url(url: &str) -> bool {
    let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    else {
        return false;
    };
    if url.contains('\\') || url.contains("/..") {
        return false;
    }
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    !host.is_empty()
}

/// manifest と同じ directory に置いた file 名だけを許す。URL・絶対パス・`..` は拒否する。
fn validate_source(source: &str) -> Result<()> {
    if source.starts_with("https://") || source.starts_with("http://") {
        return Err(Error::new(format!(
            "schema v1 は URL の source に対応していません: {source}"
        )));
    }
    if source.starts_with('/')
        || source.contains('\\')
        || source.contains(':')
        || source.contains('/')
        || source
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(Error::new(format!(
            "source が manifest の隣の file 名でない: {source}"
        )));
    }
    Ok(())
}

fn reject_unknown_and_missing(
    object: &serde_json::Map<String, Value>,
    fields: &[&str],
    label: &str,
) -> Result<()> {
    if let Some(unknown) = object.keys().find(|key| !fields.contains(&key.as_str())) {
        return Err(Error::new(format!("{label} に未知の field: {unknown}")));
    }
    if let Some(missing) = fields.iter().find(|field| !object.contains_key(**field)) {
        return Err(Error::new(format!(
            "{label} に field が足りない: {missing}"
        )));
    }
    Ok(())
}

pub(crate) fn decode_base64(encoded: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| Error::new(error.to_string()))
}

pub(crate) fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)
        .map_err(|error| Error::new(format!("file を開けません ({}): {error}", path.display())))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

pub(crate) fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
pub(crate) mod fixtures {
    use std::path::PathBuf;

    /// 配布物の fixture。`scripts.release` が実際に生成・署名したものを使う。
    pub(crate) fn directory(name: &str) -> PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(name)
    }

    pub(crate) fn keys() -> super::TrustedKeys {
        let raw = std::fs::read(directory("keys.json")).expect("fixture keys");
        super::TrustedKeys::parse(&raw).expect("fixture keys parse")
    }

    /// `directory` の内容を `destination` へ複写する。各 test が fixture を書き換えないため。
    pub(crate) fn copy(name: &str, destination: &std::path::Path) {
        for entry in std::fs::read_dir(directory(name)).expect("fixture directory") {
            let entry = entry.expect("entry");
            std::fs::copy(entry.path(), destination.join(entry.file_name())).expect("copy");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures;
    use super::*;

    fn fixture_manifest() -> Vec<u8> {
        std::fs::read(fixtures::directory("distribution").join(MANIFEST_NAME)).expect("manifest")
    }

    #[test]
    fn repository_registry_is_usable() {
        let keys = TrustedKeys::repository().expect("repository registry");
        assert!(!keys.0.is_empty());
    }

    #[test]
    fn python_signed_distribution_is_verified() {
        let distribution =
            verify_distribution(&fixtures::directory("distribution"), &fixtures::keys())
                .expect("fixture distribution verifies");
        assert_eq!(distribution.manifest.version, "0.1.0-alpha.1");
        assert!(!distribution.manifest.breaking);
        assert!(distribution.packages.contains_key("windows-x86_64"));
    }

    #[test]
    fn canonical_bytes_match_python_json() {
        let raw = fixture_manifest();
        let value: Value = serde_json::from_slice(&raw).expect("fixture json");
        let mut payload = value.as_object().expect("object").clone();
        payload.remove("signature");
        let canonical = canonical(&Value::Object(payload)).expect("canonical");
        let expected = std::fs::read(fixtures::directory("canonical-manifest.json"))
            .expect("python が署名した正規化 bytes");
        assert_eq!(canonical, expected);
    }

    #[test]
    fn embedded_registry_matches_the_repository_file() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .join("release/trusted-keys.json");
        let raw = std::fs::read(path).expect("repository registry");
        let keys = TrustedKeys::parse(&raw).expect("repository registry parses");
        let embedded = TrustedKeys::repository().expect("embedded registry");
        assert_eq!(
            keys.0.keys().collect::<Vec<_>>(),
            embedded.0.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn tampered_manifest_is_rejected() {
        let directory = tempfile::tempdir().expect("temp");
        fixtures::copy("distribution", directory.path());
        let manifest = directory.path().join(MANIFEST_NAME);
        let text = std::fs::read_to_string(&manifest).expect("manifest");
        std::fs::write(
            &manifest,
            text.replace("\"breaking\": false", "\"breaking\": true"),
        )
        .expect("tamper");
        assert!(verify_distribution(directory.path(), &fixtures::keys()).is_err());
    }

    #[test]
    fn unknown_key_is_rejected() {
        let keys = TrustedKeys::repository().expect("repository registry");
        assert!(verify_distribution(&fixtures::directory("distribution"), &keys).is_err());
    }

    #[test]
    fn tampered_package_is_rejected() {
        let directory = tempfile::tempdir().expect("temp");
        fixtures::copy("distribution", directory.path());
        let package = directory
            .path()
            .join("lapis-0.1.0-alpha.1-windows-x86_64.zip");
        let mut bytes = std::fs::read(&package).expect("package");
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        std::fs::write(&package, bytes).expect("tamper");
        assert!(verify_distribution(directory.path(), &fixtures::keys()).is_err());
    }

    #[test]
    fn schema_true_is_not_accepted_as_one() {
        let raw = br#"{"schema":true,"version":"0.1.0","breaking":false,"notes_url":"https://example.invalid","generated_at":"x","key_id":"k","assets":{"windows-x86_64":{"source":"a.zip","sha256":"0000000000000000000000000000000000000000000000000000000000000000"}},"signature":"x"}"#;
        assert!(parse(raw).is_err());
    }

    #[test]
    fn unknown_and_missing_fields_are_rejected() {
        let raw = fixture_manifest();
        let mut value: Value = serde_json::from_slice(&raw).expect("json");
        value["extra"] = Value::Bool(true);
        assert!(parse(&serde_json::to_vec(&value).expect("json")).is_err());
        let mut value: Value = serde_json::from_slice(&raw).expect("json");
        value.as_object_mut().expect("object").remove("breaking");
        assert!(parse(&serde_json::to_vec(&value).expect("json")).is_err());
    }

    #[test]
    fn source_must_be_a_file_next_to_the_manifest() {
        for source in [
            "https://example.invalid/a.zip",
            "/a.zip",
            "a\\b.zip",
            "../a.zip",
            "nested/a.zip",
            "C:/a.zip",
        ] {
            assert!(validate_source(source).is_err(), "{source}");
        }
        assert!(validate_source("lapis-0.1.0-windows-x86_64.zip").is_ok());
    }

    #[test]
    fn version_pattern_follows_the_release_schema() {
        for version in ["0.1.0", "0.1.0-alpha.1", "1.0.0.beta", "12.3"] {
            assert!(validate_version(version).is_ok(), "{version}");
        }
        for version in ["", "v0.1.0", "0.1..0", "0.1.0-", "-0.1.0"] {
            assert!(validate_version(version).is_err(), "{version}");
        }
    }

    #[test]
    fn weak_public_keys_are_rejected() {
        // 恒等元の encoding。
        let registry =
            br#"{"schema":1,"keys":{"weak":"AQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="}}"#;
        assert!(TrustedKeys::parse(registry).is_err());
    }

    #[test]
    fn registry_shape_is_checked() {
        assert!(TrustedKeys::parse(br#"{"schema":1,"keys":{},"extra":true}"#).is_err());
        assert!(TrustedKeys::parse(br#"{"schema":2,"keys":{}}"#).is_err());
        assert!(TrustedKeys::parse(br#"{"schema":1,"keys":{"k":"not base64"}}"#).is_err());
    }
}
