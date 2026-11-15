use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

/// 拡張 ID の最大 byte 数。
const MAX_ID_BYTES: usize = 64;

/// 目録 directory の中で予約する名前。拡張 ID には使わせない。
const RESERVED_IDENTIFIERS: [&str; 3] = ["catalog.json", "data", "installed"];

/// Windows が予約するデバイス名。先頭の `.` までを名前として比較する。
///
/// `NUL` などは path の要素として使うと、directory を作ったように見せながら
/// 実際には何も作らない（`create_dir_all` が成功を返しても `exists()` は false）。
/// その場合 `register` は目録だけ書き換えて、拡張データの置き場所を失う。
const WINDOWS_DEVICE_NAMES: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
/// package が宣言する拡張の識別子。配置 directory 名にそのまま使う。
///
/// 大文字を許さないのは、case を区別しないファイルシステムで別 ID が同じ
/// directory を指すのを避けるため。末尾の `.` を許さないのは、Windows が path の
/// 各要素から末尾の `.` を取り除くため。Windows の予約デバイス名も拒否する。
/// `/`・`\`・`..` なども受け付けない。
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
            || value.starts_with(crate::journal::STAGING_PREFIX)
            || RESERVED_IDENTIFIERS.contains(&value.as_str())
            || is_windows_device_name(&value)
        {
            return Err(InvalidExtensionPackageId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Windows の予約デバイス名か。`nul.txt` のように拡張子が付いていても予約される。
fn is_windows_device_name(value: &str) -> bool {
    let stem = value.split('.').next().unwrap_or_default();
    WINDOWS_DEVICE_NAMES.contains(&stem)
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
            "extension package ID must be 1 to {MAX_ID_BYTES} lowercase ASCII bytes (a-z, 0-9, '.', '-', '_'), without a trailing '.', and not a Windows device name"
        )
    }
}

impl Error for InvalidExtensionPackageId {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_prefixed_ids_are_reserved_for_leftover_cleanup() {
        // `installed/.staging-*` は回復処理が残骸として消す。その名前を拡張 ID に
        // 許すと、導入済みの実体が掃除で消える。
        for value in [".staging-a", ".staging-extension", ".staging"] {
            if value.starts_with(".staging-") {
                assert!(
                    ExtensionPackageId::try_new(value).is_err(),
                    "must reject staging prefix: {value:?}"
                );
            }
        }
        // prefix を部分文字列として含むだけの ID は使える。
        for value in ["a.staging-b", "staging-a", ".stagingx"] {
            assert!(
                ExtensionPackageId::try_new(value).is_ok(),
                "must accept {value:?}"
            );
        }
    }

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

    /// Windows は `NUL` などの予約デバイス名を path の要素に使うと、directory を
    /// 作ったように見せながら実際には何も作らない。`register` は配置を作ってから
    /// 目録を書くため、ここで拒否しないと拡張データの置き場所を失う。
    #[test]
    fn windows_device_names_are_rejected_including_names_with_an_extension() {
        for value in [
            "con",
            "prn",
            "aux",
            "nul",
            "com1",
            "com9",
            "lpt1",
            "lpt9",
            "nul.txt",
            "con.tar.gz",
        ] {
            assert!(
                ExtensionPackageId::try_new(value).is_err(),
                "must reject Windows device name: {value:?}"
            );
        }
        // 予約名を部分文字列として含むだけの ID は通常の directory 名として使える。
        for value in ["console", "a.con", "conx", "com0", "com10", "nulled"] {
            assert!(
                ExtensionPackageId::try_new(value).is_ok(),
                "must accept {value:?}"
            );
        }
    }
}
