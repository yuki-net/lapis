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
use extension_runtime::supervisor::ComponentImage;
use extensions_entry::{
    ContractId, ExtensionCatalog, ExtensionDeclarations, ExtensionLoader, ExtensionPackageId,
    HostContracts, InstallOutcome, Registration, TrustedKeys,
};

/// Host が起動時に読み込んだ拡張の目録。
pub(crate) struct HostExtensions {
    catalog: Catalog,
    loader: Loader,
    worker_executable: Option<std::path::PathBuf>,
    /// 回復に失敗して、目録と実体が食い違ったまま残っている理由。
    ///
    /// ここが `Some` の間は目録を宣言として配らない。実体の無い「導入済み」や、
    /// 実体が旧版なのに宣言が新版、という状態を Client に見せないため。
    inconsistent: Mutex<Option<String>>,
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

pub(crate) struct StartupContribution {
    pub(crate) payload: client_api::HostExtensionContributions,
    pub(crate) requires_runtime: bool,
}

impl StartupContribution {
    pub(crate) fn available(&self, active: &[String]) -> bool {
        !self.requires_runtime || active.contains(&self.payload.extension_id)
    }
}

/// ローカル管理者が指定した発行者鍵だけを組み込み鍵へ追加する。
fn trusted_keys(directory: Option<&std::path::Path>) -> Result<TrustedKeys, String> {
    use std::io::Read;
    const MAX_REGISTRY_BYTES: u64 = 1024 * 1024;
    let mut keys = TrustedKeys::repository().map_err(|error| error.to_string())?;
    let Some(directory) = directory else {
        return Ok(keys);
    };
    let path = directory.join("extensions").join("trusted-keys.json");
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(keys),
        Err(error) => return Err(error.to_string()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_REGISTRY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_REGISTRY_BYTES {
        return Err("拡張の信頼済み鍵一覧が大きすぎます".to_owned());
    }
    keys.add_registry(&bytes)
        .map_err(|error| error.to_string())?;
    Ok(keys)
}

impl HostExtensions {
    /// 起動時に有効な package の署名と内容を再検証し、その bytes を固定する。
    /// 無効化などの目録変更は次の Host 起動までこのスナップショットへ影響しない。
    pub(crate) fn startup_components(
        &self,
    ) -> (
        Vec<ComponentImage>,
        Vec<StartupContribution>,
        Vec<String>,
        Vec<String>,
        std::collections::BTreeMap<String, String>,
    ) {
        let mut failures = std::collections::BTreeMap::new();
        let loader = match self.loader() {
            Ok(loader) => loader,
            Err(error) => {
                failures.insert("host".to_owned(), error);
                return (Vec::new(), Vec::new(), Vec::new(), Vec::new(), failures);
            }
        };
        let mut catalog = match &self.catalog {
            Catalog::Absent => return (Vec::new(), Vec::new(), Vec::new(), Vec::new(), failures),
            Catalog::Unreadable(error) => {
                failures.insert("host".to_owned(), error.clone());
                return (Vec::new(), Vec::new(), Vec::new(), Vec::new(), failures);
            }
            Catalog::Ready(catalog) => match catalog.lock() {
                Ok(catalog) => catalog,
                Err(_) => {
                    failures.insert("host".to_owned(), "拡張の目録を取得できません".to_owned());
                    return (Vec::new(), Vec::new(), Vec::new(), Vec::new(), failures);
                }
            },
        };
        let ids: Vec<_> = catalog
            .entries()
            .filter(|entry| entry.state.is_installed() && entry.state.is_enabled())
            .map(|entry| entry.id.clone())
            .collect();
        let mut components = Vec::new();
        let mut contributions = Vec::new();
        let mut data_active = Vec::new();
        let mut runtime_ids = Vec::new();
        let mut total_declaration_bytes = 0usize;
        for id in ids {
            match loader.load_installed(&mut catalog, &id) {
                Ok(mut package) => {
                    let requires_runtime = package.manifest.requires.iter().any(|requirement| {
                        requirement.contract.as_str() == extension_runtime::LIFECYCLE_CONTRACT
                    });
                    let contribution = match package.files.remove("contributions.json") {
                        None => None,
                        Some(bytes) => {
                            if !package.manifest.requires.iter().any(|requirement| {
                                requirement.contract.as_str()
                                    == extension_ui_contract::STATIC_CONTRIBUTIONS_CONTRACT
                            }) {
                                failures.insert(
                                    id.as_str().to_owned(),
                                    "拡張宣言の契約が指定されていません".to_owned(),
                                );
                                continue;
                            }
                            if bytes.len() > 256 * 1024 {
                                failures.insert(
                                    id.as_str().to_owned(),
                                    "拡張宣言のサイズが上限を超えています".to_owned(),
                                );
                                continue;
                            }
                            let parsed: Result<extension_ui_contract::StaticContributions, _> =
                                serde_json::from_slice(&bytes);
                            match parsed {
                                Ok(declaration) if declaration.validate(id.as_str()).is_ok() => {
                                    let payload = client_api::HostExtensionContributions {
                                        extension_id: id.as_str().to_owned(),
                                        declaration,
                                    };
                                    let encoded_size = serde_json::to_vec(&payload)
                                        .map_or(usize::MAX, |encoded| {
                                            encoded.len().saturating_add(1)
                                        });
                                    if total_declaration_bytes.saturating_add(encoded_size)
                                        > 256 * 1024
                                    {
                                        failures.insert(
                                            id.as_str().to_owned(),
                                            "拡張宣言のサイズが上限を超えています".to_owned(),
                                        );
                                        continue;
                                    }
                                    total_declaration_bytes += encoded_size;
                                    Some(payload)
                                }
                                _ => {
                                    failures.insert(
                                        id.as_str().to_owned(),
                                        "拡張宣言が不正です".to_owned(),
                                    );
                                    continue;
                                }
                            }
                        }
                    };
                    if requires_runtime {
                        match package.files.remove("extension.wasm") {
                            Some(bytes) => {
                                runtime_ids.push(id.as_str().to_owned());
                                components.push(ComponentImage {
                                    id: id.as_str().to_owned(),
                                    bytes,
                                });
                            }
                            None => {
                                failures.insert(
                                    id.as_str().to_owned(),
                                    "ライフサイクル契約を要求する component がありません"
                                        .to_owned(),
                                );
                                continue;
                            }
                        }
                    } else {
                        data_active.push(id.as_str().to_owned());
                    }
                    if let Some(payload) = contribution {
                        contributions.push(StartupContribution {
                            payload,
                            requires_runtime,
                        });
                    }
                }
                Err(error) => {
                    failures.insert(id.as_str().to_owned(), error.to_string());
                }
            }
        }
        (
            components,
            contributions,
            data_active,
            runtime_ids,
            failures,
        )
    }
    /// 目録を開く。Host の起動は妨げない。
    ///
    /// 壊れた目録を空として扱うと、導入済み拡張の宣言を失う。上書きもしないため、
    /// 読めない間は宣言の要求にエラーを返し、file はそのまま残す。
    pub(crate) fn load(directory: Option<&std::path::Path>) -> Self {
        Self::load_with_development(directory, false)
    }

    pub(crate) fn load_with_development(
        directory: Option<&std::path::Path>,
        development: bool,
    ) -> Self {
        // 実行可能な worker の handshake を確認した場合にだけ、その実行契約を提供する。
        let worker_executable = directory
            .and_then(|_| std::env::current_exe().ok())
            .and_then(|exe| {
                exe.parent().map(|parent| {
                    parent.join(format!(
                        "lapis-extension-host{}",
                        std::env::consts::EXE_SUFFIX
                    ))
                })
            })
            .filter(|path| path.is_file())
            .filter(|path| extension_runtime::supervisor::probe_worker(path).is_ok());
        let mut contracts = HostContracts::current();
        contracts.add_version(
            ContractId::try_new(extension_ui_contract::STATIC_CONTRIBUTIONS_CONTRACT)
                .expect("valid contract id"),
            semver::Version::parse(extension_ui_contract::STATIC_CONTRIBUTIONS_VERSION)
                .expect("valid contract version"),
        );
        if worker_executable.is_some() {
            contracts.add_version(
                ContractId::try_new(extension_runtime::LIFECYCLE_CONTRACT)
                    .expect("valid contract id"),
                semver::Version::parse(extension_runtime::LIFECYCLE_VERSION)
                    .expect("valid contract version"),
            );
        }
        let loader = match trusted_keys(directory).map(|keys| {
            let loader = ExtensionLoader::new(keys, contracts);
            if development {
                loader.allow_unsigned_local_development()
            } else {
                loader
            }
        }) {
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
                worker_executable,
                inconsistent: Mutex::new(None),
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
        let mut inconsistent = None;
        let catalog = match catalog {
            Catalog::Ready(catalog) => {
                // 前回の導入・除去が中断していれば、目録を公開する前に完了／取り消しする。
                // 実体の無い「導入済み」を宣言として配らないため、回復に失敗した場合は
                // 目録を公開しない（file は残し、次の起動で再試行する）。
                let recovered = {
                    let mut guard = catalog
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    extensions_entry::recover_pending_installs(&mut guard)
                };
                match recovered {
                    Ok(_) => Catalog::Ready(catalog),
                    Err(error) => {
                        let message = format!("中断した拡張の導入を回復できません: {error}");
                        eprintln!("{message}");
                        inconsistent = Some(message.clone());
                        Catalog::Unreadable(message)
                    }
                }
            }
            other => other,
        };
        Self {
            catalog,
            loader,
            worker_executable,
            inconsistent: Mutex::new(inconsistent),
        }
    }

    pub(crate) fn worker_executable(&self) -> Option<&std::path::Path> {
        self.worker_executable.as_deref()
    }

    /// 現在の目録を宣言として返す。
    pub(crate) fn list(&self) -> Result<HostExtensionsListResponse, String> {
        // 目録と実体が食い違ったままなら、宣言を配らない。
        if let Some(reason) = self.inconsistent() {
            return Err(reason);
        }
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
                    active: None,
                    incompatible: declaration.incompatible,
                })
                .collect(),
            contributions: Vec::new(),
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
        match loader.install(&mut catalog, package, enabled) {
            Ok(outcome) => Ok(outcome),
            Err(error) => {
                self.record_recovery_failure(&error);
                Err(error.to_string())
            }
        }
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
        match loader.remove(&mut catalog, &id) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.record_recovery_failure(&error);
                Err(error.to_string())
            }
        }
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

    fn inconsistent(&self) -> Option<String> {
        self.inconsistent
            .lock()
            .ok()
            .and_then(|reason| reason.clone())
    }

    /// 回復に失敗した操作を記録し、以後の宣言公開を止める。
    ///
    /// 次に起動して回復が成功するまで、実体と宣言が食い違ったままである。
    fn record_recovery_failure(&self, error: &extensions_entry::PackageError) {
        if error.leaves_inconsistent_state()
            && let Ok(mut reason) = self.inconsistent.lock()
        {
            *reason = Some(error.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use extensions_entry::{ExtensionEntry, ExtensionState, ExtensionVersion};

    #[test]
    fn runtime_contributions_require_a_successfully_active_instance() {
        let contribution = StartupContribution {
            payload: client_api::HostExtensionContributions {
                extension_id: "dev.example.extension".into(),
                declaration: extension_ui_contract::StaticContributions {
                    schema: 1,
                    default_locale: "en-US".into(),
                    settings: Vec::new(),
                    translations: Vec::new(),
                },
            },
            requires_runtime: true,
        };
        assert!(!contribution.available(&[]));
        assert!(!contribution.available(&["other".into()]));
        assert!(contribution.available(&["dev.example.extension".into()]));
        let data_only = StartupContribution {
            requires_runtime: false,
            ..contribution
        };
        assert!(data_only.available(&[]));
    }

    #[test]
    fn explicitly_configured_publisher_keys_fail_closed_when_invalid() {
        let directory = tempfile::tempdir().unwrap();
        let registry_dir = directory.path().join("extensions");
        std::fs::create_dir_all(&registry_dir).unwrap();
        assert!(trusted_keys(Some(directory.path())).is_ok());
        std::fs::write(
            registry_dir.join("trusted-keys.json"),
            br#"{"schema":1,"keys":{}}"#,
        )
        .unwrap();
        assert!(trusted_keys(Some(directory.path())).is_ok());
        std::fs::write(registry_dir.join("trusted-keys.json"), b"not json").unwrap();
        assert!(trusted_keys(Some(directory.path())).is_err());
        let host = HostExtensions::load(Some(directory.path()));
        assert!(host.loader().is_err());
        assert!(host.list().is_ok());
    }

    fn entry(id: &str, enabled: bool) -> ExtensionEntry {
        ExtensionEntry::new(
            ExtensionPackageId::try_new(id).unwrap(),
            ExtensionVersion::parse("0.1.0").unwrap(),
            ExtensionState::installed(enabled),
        )
    }

    #[test]
    fn an_unrecovered_error_stops_the_declaration_list() {
        let directory = tempfile::tempdir().unwrap();
        let extensions = HostExtensions::load(Some(directory.path()));
        assert!(extensions.list().is_ok());

        // 回復に失敗した操作を記録すると、以後の宣言公開を止める。
        extensions.record_recovery_failure(&extensions_entry::PackageError::Unrecovered {
            cause: "配置に失敗".to_owned(),
            recovery: "旧版を戻せない".to_owned(),
        });
        assert!(
            extensions.list().is_err(),
            "実体と宣言が食い違ったまま宣言を配らない"
        );
    }

    #[test]
    fn an_ordinary_install_error_keeps_publishing_the_declaration() {
        let directory = tempfile::tempdir().unwrap();
        let extensions = HostExtensions::load(Some(directory.path()));
        // 目録を変更していない失敗では、宣言の公開を止めない。
        extensions.record_recovery_failure(&extensions_entry::PackageError::Malformed(
            "manifest がない".to_owned(),
        ));
        assert!(extensions.list().unwrap().extensions.is_empty());
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
                    active: None,
                    incompatible: false,
                },
                HostExtension {
                    id: "dev.example.two".to_owned(),
                    version: "0.1.0".to_owned(),
                    installed: true,
                    enabled: false,
                    active: None,
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
