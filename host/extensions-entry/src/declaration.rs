//! 目録を Client へ見せる宣言へ集約する。
//!
//! 宣言は目録の現在値をそのまま写した読み取り専用の値であり、Client は表示にだけ使う。
//! 同じ正本を Client 側へ持たせないため、ここでは cache・購読・差分を持たない。

use crate::{ExtensionCatalog, ExtensionEntry, ExtensionState};

/// 拡張 1 件の宣言。
///
/// `installed` / `enabled` / `incompatible` は目録の状態をそのまま写す。`incompatible`
/// は「package は読めたが互換性がなく、実体を置いていない」ことを表し、このとき
/// `installed` と `enabled` はどちらも false になる。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtensionDeclaration {
    pub id: String,
    pub version: String,
    pub installed: bool,
    pub enabled: bool,
    pub incompatible: bool,
}

/// 目録の全 declaration。`id` の順に並べる。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtensionDeclarations {
    pub extensions: Vec<ExtensionDeclaration>,
}

impl ExtensionDeclarations {
    /// 目録の現在値を宣言へ集約する。
    pub fn from_catalog(catalog: &ExtensionCatalog) -> Self {
        Self {
            extensions: catalog.entries().map(declaration).collect(),
        }
    }

    pub fn find(&self, id: &str) -> Option<&ExtensionDeclaration> {
        self.extensions
            .iter()
            .find(|declaration| declaration.id == id)
    }

    /// 実際に読み込まれた（実体があり、有効な）拡張の ID。
    ///
    /// Wasm 実行は将来段階であり、ここでは「読み込んでよい拡張」の集合だけを返す。
    pub fn loaded(&self) -> impl Iterator<Item = &ExtensionDeclaration> {
        self.extensions
            .iter()
            .filter(|declaration| declaration.installed && declaration.enabled)
    }
}

fn declaration(entry: &ExtensionEntry) -> ExtensionDeclaration {
    let state: ExtensionState = entry.state;
    ExtensionDeclaration {
        id: entry.id.as_str().to_owned(),
        version: entry.version.as_str().to_owned(),
        installed: state.is_installed(),
        enabled: state.is_enabled(),
        incompatible: state.is_incompatible(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExtensionPackageId, ExtensionVersion};

    fn id(value: &str) -> ExtensionPackageId {
        ExtensionPackageId::try_new(value).unwrap()
    }

    fn version(value: &str) -> ExtensionVersion {
        ExtensionVersion::parse(value).unwrap()
    }

    #[test]
    fn declarations_mirror_the_catalog_state() {
        let directory = tempfile::tempdir().unwrap();
        let mut catalog = ExtensionCatalog::open(directory.path().join("extensions")).unwrap();
        catalog
            .register(ExtensionEntry::new(
                id("dev.example.one"),
                version("0.1.0"),
                ExtensionState::installed(true),
            ))
            .unwrap();
        catalog
            .register(ExtensionEntry::new(
                id("dev.example.two"),
                version("0.2.0"),
                ExtensionState::installed(false),
            ))
            .unwrap();
        for state in [
            ExtensionState::unavailable(),
            ExtensionState::incompatible(),
        ] {
            catalog
                .register(ExtensionEntry::new(
                    id(if state.is_incompatible() {
                        "dev.example.four"
                    } else {
                        "dev.example.three"
                    }),
                    version("0.3.0"),
                    state,
                ))
                .unwrap();
        }

        let declarations = ExtensionDeclarations::from_catalog(&catalog);
        assert_eq!(declarations.extensions.len(), 4);
        let one = declarations.find("dev.example.one").unwrap();
        assert!(one.installed && one.enabled && !one.incompatible);
        let two = declarations.find("dev.example.two").unwrap();
        assert!(two.installed && !two.enabled);
        let three = declarations.find("dev.example.three").unwrap();
        assert!(!three.installed && !three.enabled && !three.incompatible);
        let four = declarations.find("dev.example.four").unwrap();
        assert!(!four.installed && !four.enabled && four.incompatible);
        assert_eq!(
            declarations
                .loaded()
                .map(|declaration| declaration.id.as_str())
                .collect::<Vec<_>>(),
            vec!["dev.example.one"],
            "有効な拡張だけを読み込み対象にする"
        );
    }
}
