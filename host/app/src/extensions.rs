//! 拡張の入口を Host の公開操作へ接続する。
//!
//! 目録は Host のデータ領域にだけ置き、Client へは現在値を宣言として返す。
//! `host.extensions.list` は `host.manage` を要求する読み取りで、状態を変えない。

use std::sync::Mutex;

use client_api::{HostExtension, HostExtensionsListResponse};
use extensions_entry::ExtensionCatalog;

/// Host が起動時に読み込んだ拡張の目録。
pub(crate) struct HostExtensions {
    catalog: Catalog,
}

enum Catalog {
    /// state directory を持たない Host。拡張も持たない。
    Absent,
    Ready(Mutex<ExtensionCatalog>),
    /// 目録を読めなかった。file は残し、宣言の要求にだけエラーを返す。
    Unreadable(String),
}

impl HostExtensions {
    /// 目録を開く。Host の起動は妨げない。
    ///
    /// 壊れた目録を空として扱うと、導入済み拡張の宣言を失う。上書きもしないため、
    /// 読めない間は宣言の要求にエラーを返し、file はそのまま残す。
    pub(crate) fn load(directory: Option<&std::path::Path>) -> Self {
        let Some(directory) = directory else {
            return Self {
                catalog: Catalog::Absent,
            };
        };
        let catalog = match ExtensionCatalog::open(directory.join("extensions")) {
            Ok(catalog) => Catalog::Ready(Mutex::new(catalog)),
            Err(error) => {
                let message = error.to_string();
                eprintln!("拡張の目録を読めません: {message}");
                Catalog::Unreadable(message)
            }
        };
        Self { catalog }
    }

    /// 現在の目録を宣言として返す。
    pub(crate) fn list(&self) -> Result<HostExtensionsListResponse, String> {
        let catalog = match &self.catalog {
            Catalog::Absent => return Ok(HostExtensionsListResponse::default()),
            Catalog::Ready(catalog) => catalog,
            Catalog::Unreadable(message) => return Err(message.clone()),
        };
        let catalog = catalog
            .lock()
            .map_err(|_| "拡張の目録を取得できません".to_owned())?;
        Ok(HostExtensionsListResponse {
            extensions: catalog
                .entries()
                .map(|entry| HostExtension {
                    id: entry.id.as_str().to_owned(),
                    version: entry.version.as_str().to_owned(),
                    installed: entry.state.is_installed(),
                    enabled: entry.state.is_enabled(),
                })
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use extensions_entry::{ExtensionEntry, ExtensionPackageId, ExtensionState, ExtensionVersion};

    fn entry(id: &str, enabled: bool) -> ExtensionEntry {
        ExtensionEntry::new(
            ExtensionPackageId::try_new(id).unwrap(),
            ExtensionVersion::parse("0.1.0").unwrap(),
            ExtensionState::installed(enabled),
        )
    }

    #[test]
    fn a_host_without_a_state_directory_declares_no_extensions() {
        let extensions = HostExtensions::load(None);
        assert!(extensions.list().unwrap().extensions.is_empty());
    }

    #[test]
    fn the_declaration_reports_installed_and_enabled_separately() {
        let directory = tempfile::tempdir().unwrap();
        let mut catalog = ExtensionCatalog::open(directory.path().join("extensions")).unwrap();
        catalog.register(entry("dev.example.one", true)).unwrap();
        catalog.register(entry("dev.example.two", false)).unwrap();
        drop(catalog);

        let list = HostExtensions::load(Some(directory.path())).list().unwrap();
        assert_eq!(
            list.extensions,
            vec![
                HostExtension {
                    id: "dev.example.one".to_owned(),
                    version: "0.1.0".to_owned(),
                    installed: true,
                    enabled: true,
                },
                HostExtension {
                    id: "dev.example.two".to_owned(),
                    version: "0.1.0".to_owned(),
                    installed: true,
                    enabled: false,
                },
            ]
        );
    }

    #[test]
    fn an_unreadable_catalog_reports_an_error_and_is_not_replaced() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("extensions");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("catalog.json"), b"broken").unwrap();

        let extensions = HostExtensions::load(Some(directory.path()));
        assert!(extensions.list().is_err());
        assert_eq!(std::fs::read(root.join("catalog.json")).unwrap(), b"broken");
    }
}
