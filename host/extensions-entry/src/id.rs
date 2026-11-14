use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

/// 拡張 ID の最大 byte 数。
const MAX_ID_BYTES: usize = 64;

/// 目録 directory の中で予約する名前。拡張 ID には使わせない。
const RESERVED_IDENTIFIERS: [&str; 3] = ["catalog.json", "data", "installed"];

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
/// package が宣言する拡張の識別子。配置 directory 名にそのまま使う。
///
/// 大文字を許さないのは、case を区別しないファイルシステムで別 ID が同じ
/// directory を指すのを避けるため。末尾の `.` を許さないのは、Windows が path の
/// 各要素から末尾の `.` を取り除くため。`/`・`\`・`..` なども受け付けない。
pub struct ExtensionPackageId(String);

impl ExtensionPackageId {
    pub fn try_new(value: impl Into<String>) -> Result<Self, InvalidExtensionPackageId> {
        let value = value.into();
        // 末尾の `.` を拒否する。Windows は path の各要素から末尾の `.` と空白を
        // 取り除いて扱うため、`a.` と `a` が同じ directory を指してしまう。配置先は
        // ID をそのまま directory 名にするので、ここで断つ。`.` と `..` も
        // 末尾 `.` に含まれる。空白は許可する文字に含まれないため、末尾の空白は
        // この条件より前に弾かれる。
        if value.is_empty()
            || value.len() > MAX_ID_BYTES
            || !value
                .bytes()
                .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-' | b'_'))
            || value.ends_with('.')
            || RESERVED_IDENTIFIERS.contains(&value.as_str())
        {
            return Err(InvalidExtensionPackageId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ExtensionPackageId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::try_new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for ExtensionPackageId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 拡張 ID が空・長すぎる・配置に使えない文字を含む。
pub struct InvalidExtensionPackageId;

impl fmt::Display for InvalidExtensionPackageId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "extension package ID must be 1 to {MAX_ID_BYTES} lowercase ASCII bytes (a-z, 0-9, '.', '-', '_') without a trailing '.'"
        )
    }
}

impl Error for InvalidExtensionPackageId {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_lowercase_path_safe_and_round_trip_as_strings() {
        let id = ExtensionPackageId::try_new("dev.example.terminal").unwrap();
        assert_eq!(id.as_str(), "dev.example.terminal");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, r#""dev.example.terminal""#);
        assert_eq!(
            serde_json::from_str::<ExtensionPackageId>(&json).unwrap(),
            id
        );
    }

    #[test]
    fn ids_that_escape_or_collide_on_a_case_insensitive_filesystem_are_rejected() {
        for value in [
            "",
            " ",
            "Dev.Example",
            "..",
            ".",
            "a/b",
            "a\\b",
            "a:b",
            "dev/../escape",
            "installed",
            "data",
            "catalog.json",
            " leading",
        ] {
            assert!(
                ExtensionPackageId::try_new(value).is_err(),
                "must reject {value:?}"
            );
        }
        assert!(ExtensionPackageId::try_new("a".repeat(MAX_ID_BYTES + 1)).is_err());
    }

    /// Windows は path の各要素から末尾の `.` を取り除くため、`a.` と `a` が同じ
    /// directory を指す。配置先は ID をそのまま directory 名にするので、末尾 `.` を
    /// 受け入れると別の拡張が同じ場所を共有してしまう。
    #[test]
    fn ids_with_a_trailing_dot_are_rejected_on_every_platform() {
        for value in ["a.", "a..", "dev.example.terminal.", "1.", "a.b."] {
            assert!(
                ExtensionPackageId::try_new(value).is_err(),
                "must reject trailing dot: {value:?}"
            );
        }
        // 末尾以外の `.` は区切りとして使い続けられる。
        for value in ["a.b", "a-b", "a_b", "a.1", "1.a"] {
            assert!(
                ExtensionPackageId::try_new(value).is_ok(),
                "must accept {value:?}"
            );
        }
    }

    #[test]
    fn deserialization_rejects_invalid_ids() {
        assert!(serde_json::from_str::<ExtensionPackageId>(r#""Bad/Id""#).is_err());
        assert!(serde_json::from_str::<ExtensionPackageId>(r#""a.""#).is_err());
    }
}
