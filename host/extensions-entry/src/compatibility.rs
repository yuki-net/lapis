//! 拡張が要求する契約と版の範囲を、Host が提供する版と突き合わせる。
//!
//! manifest は契約ごとに「この範囲の版で動く」と宣言し、Host は自分が提供する
//! 契約の版を持つ。1 つでも満たさなければ package を読み込まず、非互換として扱う。
//! 範囲は両端を含む（`minimum <= host <= maximum`）。

use std::{collections::BTreeMap, error::Error, fmt};

use semver::Version;
use serde::Deserialize;

/// Host が拡張へ提供する package 形式の契約。manifest・ファイル照合・配置の意味。
pub const EXTENSION_CONTRACT: &str = "lapis.extension";

/// Host が提供する [`EXTENSION_CONTRACT`] の版。
///
/// 契約の意味を変えるときはこの値を上げ、旧版しか知らない拡張を非互換にする。
pub const HOST_EXTENSION_CONTRACT_VERSION: (u64, u64, u64) = (1, 0, 0);

const MAX_CONTRACT_ID_BYTES: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
/// manifest が要求する契約の識別子。
pub struct ContractId(String);

impl ContractId {
    pub fn try_new(value: impl Into<String>) -> Result<Self, InvalidContractId> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_CONTRACT_ID_BYTES
            || !value
                .bytes()
                .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-' | b'_'))
            || value.starts_with('.')
            || value.ends_with('.')
        {
            return Err(InvalidContractId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ContractId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::try_new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for ContractId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 契約 ID が空・長すぎる・使えない文字を含む。
pub struct InvalidContractId;

impl fmt::Display for InvalidContractId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "contract ID must be 1 to {MAX_CONTRACT_ID_BYTES} lowercase ASCII bytes (a-z, 0-9, '.', '-', '_')"
        )
    }
}

impl Error for InvalidContractId {}

/// manifest が 1 つの契約に要求する版の範囲。両端を含む。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContractRequirement {
    pub contract: ContractId,
    pub minimum: Version,
    pub maximum: Version,
}

impl ContractRequirement {
    pub fn new(
        contract: ContractId,
        minimum: Version,
        maximum: Version,
    ) -> Result<Self, InvalidContractRequirement> {
        // SemVer の precedence で比べる。`Version` の既定順序は build metadata も比べるため、
        // `1.0.0+a` から `1.0.0+b` を逆転と誤判定する。
        if minimum.cmp_precedence(&maximum).is_gt() {
            return Err(InvalidContractRequirement);
        }
        Ok(Self {
            contract,
            minimum,
            maximum,
        })
    }

    fn contains(&self, provided: &Version) -> bool {
        // build metadata は兼容性に影響しない（SemVer の precedence）。`1.0.0+vendor` を
        // 要求する package を、Host の `1.0.0` と一致させる。
        self.minimum.cmp_precedence(provided).is_le()
            && provided.cmp_precedence(&self.maximum).is_le()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 要求する版の範囲が逆転している（`minimum > maximum`）。
pub struct InvalidContractRequirement;

impl fmt::Display for InvalidContractRequirement {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("contract minimum version must not exceed maximum version")
    }
}

impl Error for InvalidContractRequirement {}

/// Host が提供する契約と版。読み込み時の照合にだけ使う。
#[derive(Clone, Debug)]
pub struct HostContracts {
    provided: BTreeMap<ContractId, Version>,
}

impl Default for HostContracts {
    fn default() -> Self {
        Self::current()
    }
}

impl HostContracts {
    /// この Host が提供する契約。package 形式の契約だけを持つ。
    pub fn current() -> Self {
        let (major, minor, patch) = HOST_EXTENSION_CONTRACT_VERSION;
        let version = Version::new(major, minor, patch);
        Self {
            provided: BTreeMap::from([(
                ContractId::try_new(EXTENSION_CONTRACT).expect("valid contract id"),
                version,
            )]),
        }
    }

    /// 提供する契約を明示して作る。検証規則を確かめるテストが使う。
    pub fn new(provided: impl IntoIterator<Item = (ContractId, Version)>) -> Self {
        Self {
            provided: provided.into_iter().collect(),
        }
    }

    /// 要求された契約と版の範囲をすべて満たすか。
    pub fn check(&self, requirements: &[ContractRequirement]) -> Result<(), Incompatibility> {
        for requirement in requirements {
            let Some(provided) = self.provided.get(&requirement.contract) else {
                return Err(Incompatibility::UnprovidedContract {
                    contract: requirement.contract.clone(),
                });
            };
            if !requirement.contains(provided) {
                return Err(Incompatibility::out_of_range(requirement, provided));
            }
        }
        Ok(())
    }
}

/// 互換照合に失敗した理由。読み込まない判断と、その理由の提示に使う。
///
/// `Version` をそのまま持つと error 型が大きくなり、`Result` の戻り値が膨らむ。
/// 版は表示にだけ使うため box して保持する。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Incompatibility {
    /// Host が提供していない契約を要求している。
    UnprovidedContract { contract: ContractId },
    /// Host の版が要求された範囲の外。
    VersionOutOfRange {
        contract: ContractId,
        minimum: Box<Version>,
        maximum: Box<Version>,
        provided: Box<Version>,
    },
}

impl Incompatibility {
    fn out_of_range(requirement: &ContractRequirement, provided: &Version) -> Self {
        Self::VersionOutOfRange {
            contract: requirement.contract.clone(),
            minimum: Box::new(requirement.minimum.clone()),
            maximum: Box::new(requirement.maximum.clone()),
            provided: Box::new(provided.clone()),
        }
    }
}

impl fmt::Display for Incompatibility {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnprovidedContract { contract } => {
                write!(
                    formatter,
                    "Host が提供しない契約を要求しています: {contract}"
                )
            }
            Self::VersionOutOfRange {
                contract,
                minimum,
                maximum,
                provided,
            } => write!(
                formatter,
                "契約 {contract} は {minimum} 以上 {maximum} 以下が必要ですが、Host は {provided} です"
            ),
        }
    }
}

impl Error for Incompatibility {}

#[cfg(test)]
mod tests {
    use super::*;

    fn contract(value: &str) -> ContractId {
        ContractId::try_new(value).unwrap()
    }

    fn version(value: &str) -> Version {
        Version::parse(value).unwrap()
    }

    fn requirement(contract_id: &str, minimum: &str, maximum: &str) -> ContractRequirement {
        ContractRequirement::new(contract(contract_id), version(minimum), version(maximum)).unwrap()
    }

    #[test]
    fn the_current_host_provides_the_package_contract() {
        let host = HostContracts::current();
        assert!(
            host.check(&[requirement(EXTENSION_CONTRACT, "1.0.0", "1.0.0")])
                .is_ok()
        );
        assert!(
            host.check(&[requirement(EXTENSION_CONTRACT, "0.9.0", "1.2.0")])
                .is_ok()
        );
    }

    #[test]
    fn out_of_range_and_unprovided_contracts_are_incompatible() {
        let host = HostContracts::current();
        assert!(matches!(
            host.check(&[requirement(EXTENSION_CONTRACT, "2.0.0", "2.9.0")]),
            Err(Incompatibility::VersionOutOfRange { .. })
        ));
        // 範囲の端を含む。1.0.0 は Host の版であり、上限側の比較を落とさない。
        assert!(
            host.check(&[requirement(EXTENSION_CONTRACT, "0.1.0", "1.0.0")])
                .is_ok()
        );
        assert!(matches!(
            host.check(&[requirement("lapis.extension.wasm", "1.0.0", "1.0.0")]),
            Err(Incompatibility::UnprovidedContract { .. })
        ));
        // 1 つでも満たさなければ非互換。
        assert!(
            host.check(&[
                requirement(EXTENSION_CONTRACT, "1.0.0", "1.0.0"),
                requirement("lapis.unknown", "1.0.0", "1.0.0"),
            ])
            .is_err()
        );
    }

    #[test]
    fn contract_ids_are_lowercase_path_safe() {
        for value in ["lapis.extension", "a.b-c_d", "x"] {
            assert!(ContractId::try_new(value).is_ok(), "{value}");
        }
        for value in ["", " ", "Lapis.Extension", ".a", "a.", "a/b", "a\\b", "a:b"] {
            assert!(ContractId::try_new(value).is_err(), "{value}");
        }
        assert!(ContractId::try_new("a".repeat(MAX_CONTRACT_ID_BYTES + 1)).is_err());
    }

    #[test]
    fn reversed_ranges_are_rejected() {
        assert!(
            ContractRequirement::new(
                contract(EXTENSION_CONTRACT),
                version("1.1.0"),
                version("1.0.0")
            )
            .is_err()
        );
    }

    #[test]
    fn build_metadata_does_not_affect_compatibility() {
        let host = HostContracts::current();
        // SemVer の precedence は build metadata を無視する。`1.0.0+vendor` を要求する
        // package を、Host の `1.0.0` と互換として扱う。
        assert!(
            host.check(&[requirement(
                EXTENSION_CONTRACT,
                "1.0.0+vendor",
                "1.0.0+vendor"
            )])
            .is_ok()
        );
        assert!(
            host.check(&[requirement(EXTENSION_CONTRACT, "1.0.0+a", "1.0.0+b")])
                .is_ok(),
            "build metadata だけが違う範囲を逆転と判定しない"
        );
        // precedence が違えば従来どおり非互換。
        assert!(
            host.check(&[requirement(
                EXTENSION_CONTRACT,
                "1.0.1+vendor",
                "1.0.1+vendor"
            )])
            .is_err()
        );
    }
}
