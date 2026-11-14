use std::{error, fmt};

use serde::{Deserialize, Serialize};

const MAX_VERSION_BYTES: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
/// 導入済み拡張の版。表示と比較だけに使い、順序の解釈（semver）はしない。
pub struct ExtensionVersion(String);

impl ExtensionVersion {
    pub fn parse(value: impl Into<String>) -> Result<Self, InvalidExtensionVersion> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_VERSION_BYTES
            || !value.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(InvalidExtensionVersion);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ExtensionVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for ExtensionVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 版が空・長すぎる・空白などの制御文字を含む。
pub struct InvalidExtensionVersion;

impl fmt::Display for InvalidExtensionVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "extension version must be 1 to {MAX_VERSION_BYTES} printable ASCII characters"
        )
    }
}

impl error::Error for InvalidExtensionVersion {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_round_trip_and_reject_unusable_values() {
        let version = ExtensionVersion::parse("0.2.0-alpha.1").unwrap();
        assert_eq!(version.as_str(), "0.2.0-alpha.1");
        let json = serde_json::to_string(&version).unwrap();
        assert_eq!(json, r#""0.2.0-alpha.1""#);
        assert_eq!(
            serde_json::from_str::<ExtensionVersion>(&json).unwrap(),
            version
        );

        for value in ["", " ", "0.1.0 beta", "0.1.0\n"] {
            assert!(
                ExtensionVersion::parse(value).is_err(),
                "must reject {value:?}"
            );
        }
        assert!(ExtensionVersion::parse("1".repeat(MAX_VERSION_BYTES + 1)).is_err());
    }
}
