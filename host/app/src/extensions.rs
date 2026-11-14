//! 拡張の入口を Host の公開操作へ接続する。
//!
//! 目録は Host のデータ領域にだけ置き、Client へは現在値を宣言として返す。
//! `host.extensions.list` は `extensions.read` を要求する読み取りで、状態を変えない。
//!
//! `.lapis` package の読み込みと有効化・無効化は Host の管理操作であり、標準入力から
//! 行う（`host/README.md`）。反映は入口の再起動で行うため、読み込み・状態変更は目録の
//! 宣言を書き換えるだけにする。

use std::sync::Mutex;

use client_api::{HostExtension, HostExtensionsListResponse};
use extensions_entry::{
    ExtensionCatalog, ExtensionDeclarations, ExtensionLoader, ExtensionPackageId, InstallOutcome,
    Registration,
};

/// Host が起動時に読み込んだ拡張の目録。
pub(crate) struct HostExtensions {
    catalog: Catalog,
    loader: Loader,
}

enum Catalog {
    /// state directory を持たない Host。拡張も持たない。
    Absent,
    Ready(Mutex<ExtensionCatalog>),
    /// 目録を読めなかった。file は残し、宣言の要求にだけエラーを返す。
    Unreadable(String),
}

enum Loader {
    Ready(ExtensionLoader),
    /// 信頼済み公開鍵を読めなかった。目録の読み取りは続け、読み込みだけを拒否する。
    Unavailable(String),
}

impl HostExtensions {
    /// 目録を開く。Host の起動は妨げない。
    ///
    /// 壊れた目録を空として扱うと、導入済み拡張の宣言を失う。上書きもしないため、
    /// 読めない間は宣言の要求にエラーを返し、file はそのまま残す。
    pub(crate) fn load(directory: Option<&std::path::Path>) -> Self {
        let loader = match ExtensionLoader::repository() {
            Ok(loader) => Loader::Ready(loader),
            Err(error) => {
                let message = error.to_string();
                eprintln!("拡張の署名鍵を読めないため、package を読み込めません: {message}");
                Loader::Unavailable(message)
            }
        };
        let Some(directory) = directory else {
            return Self {
                catalog: Catalog::Absent,
                loader,
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
        Self { catalog, loader }
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
        // 宣言の集約は crate の `ExtensionDeclarations` を正本にし、Host 側で写しを
        // 二重に作らない。
        let declarations = ExtensionDeclarations::from_catalog(&catalog);
        Ok(HostExtensionsListResponse {
            extensions: declarations
                .extensions
                .iter()
                .map(|declaration| HostExtension {
                    id: declaration.id.clone(),
                    version: declaration.version.clone(),
                    installed: declaration.installed,
                    enabled: declaration.enabled,
                    incompatible: declaration.incompatible,
                })
                .collect(),
        })
    }

    /// `.lapis` package を読み込み、目録へ宣言を登録する。
    pub(crate) fn install(
        &self,
        package: &std::path::Path,
        enabled: bool,
    ) -> Result<InstallOutcome, String> {
        let loader = self.loader()?;
        let mut catalog = self.catalog_mut()?;
        loader
            .install(&mut catalog, package, enabled)
            .map_err(|error| error.to_string())
    }

    /// 有効化・無効化を目録の宣言として登録する。`None` は変更できない対象。
    pub(crate) fn set_enabled(
        &self,
        id: &str,
        enabled: bool,
    ) -> Result<Option<Registration>, String> {
        let id = ExtensionPackageId::try_new(id).map_err(|error| error.to_string())?;
        let loader = self.loader()?;
        let mut catalog = self.catalog_mut()?;
        loader
            .set_enabled(&mut catalog, &id, enabled)
            .map_err(|error| error.to_string())
    }

    /// 導入済み package を取り除く。拡張データは残す。
    pub(crate) fn remove(&self, id: &str) -> Result<(), String> {
        let id = ExtensionPackageId::try_new(id).map_err(|error| error.to_string())?;
        let loader = self.loader()?;
        let mut catalog = self.catalog_mut()?;
        loader
            .remove(&mut catalog, &id)
            .map_err(|error| error.to_string())
    }

    fn catalog_mut(&self) -> Result<std::sync::MutexGuard<'_, ExtensionCatalog>, String> {
        match &self.catalog {
            Catalog::Absent => Err("この Host は拡張の状態ディレクトリを持ちません".to_owned()),
            Catalog::Ready(catalog) => catalog
                .lock()
                .map_err(|_| "拡張の目録を取得できません".to_owned()),
            Catalog::Unreadable(message) => Err(message.clone()),
        }
    }

    fn loader(&self) -> Result<&ExtensionLoader, String> {
        match &self.loader {
            Loader::Ready(loader) => Ok(loader),
            Loader::Unavailable(message) => Err(message.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use extensions_entry::{ExtensionEntry, ExtensionState, ExtensionVersion};

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
        assert!(
            extensions
                .install(std::path::Path::new("missing.lapis"), true)
                .is_err(),
            "state directory が無ければ package を読み込まない"
        );
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
                    incompatible: false,
                },
                HostExtension {
                    id: "dev.example.two".to_owned(),
                    version: "0.1.0".to_owned(),
                    installed: true,
                    enabled: false,
                    incompatible: false,
                },
            ]
        );
    }

    #[test]
    fn the_declaration_marks_incompatible_packages_apart_from_missing_ones() {
        let directory = tempfile::tempdir().unwrap();
        let mut catalog = ExtensionCatalog::open(directory.path().join("extensions")).unwrap();
        catalog
            .register(ExtensionEntry::new(
                ExtensionPackageId::try_new("dev.example.future").unwrap(),
                ExtensionVersion::parse("1.0.0").unwrap(),
                ExtensionState::incompatible(),
            ))
            .unwrap();
        drop(catalog);

        let list = HostExtensions::load(Some(directory.path())).list().unwrap();
        let declaration = &list.extensions[0];
        assert!(declaration.incompatible);
        assert!(!declaration.installed);
        assert!(!declaration.enabled);
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

    #[test]
    fn enabling_a_missing_extension_changes_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let extensions = HostExtensions::load(Some(directory.path()));
        assert!(extensions.list().unwrap().extensions.is_empty());
        assert!(
            extensions
                .set_enabled("dev.example.none", true)
                .unwrap()
                .is_none()
        );
        assert!(extensions.list().unwrap().extensions.is_empty());
    }
}
