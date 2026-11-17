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
    runtime_packages: Mutex<std::collections::BTreeMap<String, extensions_entry::VerifiedPackage>>,
    permissions_directory: Option<std::path::PathBuf>,
    #[cfg(windows)]
    supports_pty: bool,
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

type StartupExtensions = (
    Vec<ComponentImage>,
    Vec<StartupContribution>,
    Vec<String>,
    Vec<String>,
    std::collections::BTreeMap<String, String>,
);

#[cfg(windows)]
fn declares_pty_contract(package: &extensions_entry::VerifiedPackage) -> bool {
    let version = semver::Version::new(1, 0, 0);
    package.manifest.requires.iter().any(|requirement| {
        requirement.contract.as_str() == "lapis.process.pty"
            && requirement.minimum.cmp_precedence(&version).is_le()
            && requirement.maximum.cmp_precedence(&version).is_ge()
    })
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
    /// 同梱物も通常と同じ署名・互換性検証を通し、初回だけ導入する。
    pub(crate) fn install_bundled(&self, directory: &std::path::Path) -> Result<(), String> {
        let entries = match std::fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.to_string()),
        };
        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            let kind = entry.file_type().map_err(|error| error.to_string())?;
            if kind.is_file() && entry.path().extension().is_some_and(|ext| ext == "lapis") {
                paths.push(entry.path());
                if paths.len() > 64 {
                    return Err("同梱拡張が多すぎます".to_owned());
                }
            }
        }
        paths.sort();
        if paths.is_empty() {
            return Ok(());
        }
        let loader = self.loader()?;
        let mut catalog = self.catalog_mut()?;
        let mut failures = Vec::new();
        for path in paths {
            if let Err(error) = loader.install_if_missing(&mut catalog, &path, true) {
                self.record_recovery_failure(&error);
                if error.leaves_inconsistent_state() {
                    return Err(error.to_string());
                }
                failures.push(error.to_string());
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("; "))
        }
    }
    /// 起動時に有効な package の署名と内容を再検証し、その bytes を固定する。
    /// 無効化などの目録変更は次の Host 起動までこのスナップショットへ影響しない。
    pub(crate) fn startup_components(&self) -> StartupExtensions {
        if let Ok(mut packages) = self.runtime_packages.lock() {
            packages.clear();
        }
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
                                    require_dispatch: package.manifest.requires.iter().any(
                                        |requirement| {
                                            requirement.contract.as_str()
                                                == extension_runtime::DISPATCH_CONTRACT
                                        },
                                    ),
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
                    if requires_runtime {
                        // 認可に必要なのは署名検証済み manifest の識別情報だけ。
                        package.files.clear();
                        if let Ok(mut packages) = self.runtime_packages.lock() {
                            packages.insert(id.as_str().to_owned(), package);
                        }
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
        let worker = directory
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
            .and_then(|path| {
                extension_runtime::supervisor::probe_worker(&path)
                    .ok()
                    .map(|capabilities| (path, capabilities))
            });
        let supports_dispatch = worker.as_ref().is_some_and(|(_, capabilities)| {
            capabilities.dispatch_version.as_deref() == Some(extension_runtime::DISPATCH_VERSION)
        });
        #[cfg(windows)]
        let supports_pty = worker.as_ref().is_some_and(|(_, capabilities)| {
            capabilities.pty_version.as_deref() == Some(extension_runtime::PTY_VERSION)
        });
        let worker_executable = worker.map(|(path, _)| path);
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
            if supports_dispatch {
                contracts.add_version(
                    ContractId::try_new(extension_runtime::DISPATCH_CONTRACT)
                        .expect("valid contract id"),
                    semver::Version::parse(extension_runtime::DISPATCH_VERSION)
                        .expect("valid contract version"),
                );
            }
            #[cfg(windows)]
            if supports_pty {
                contracts.add_version(
                    ContractId::try_new("lapis.process.pty").expect("valid contract id"),
                    semver::Version::new(1, 0, 0),
                );
            }
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
                runtime_packages: Mutex::new(std::collections::BTreeMap::new()),
                permissions_directory: None,
                #[cfg(windows)]
                supports_pty,
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
            runtime_packages: Mutex::new(std::collections::BTreeMap::new()),
            permissions_directory: Some(directory.join("extensions")),
            #[cfg(windows)]
            supports_pty,
        }
    }

    pub(crate) fn worker_executable(&self) -> Option<&std::path::Path> {
        self.worker_executable.as_deref()
    }

    /// インストール後の変更ではなく、現在実行中の署名済み manifest を参照する。
    pub(crate) fn declares_dispatch(&self, id: &str) -> Result<bool, String> {
        let packages = self
            .runtime_packages
            .lock()
            .map_err(|error| error.to_string())?;
        let package = packages.get(id).ok_or("拡張が実行されていません")?;
        let version = semver::Version::parse(extension_runtime::DISPATCH_VERSION)
            .expect("valid contract version");
        Ok(package.manifest.requires.iter().any(|requirement| {
            requirement.contract.as_str() == extension_runtime::DISPATCH_CONTRACT
                && requirement.minimum.cmp_precedence(&version).is_le()
                && requirement.maximum.cmp_precedence(&version).is_ge()
        }))
    }

    /// 起動時に検証した package の能力承認を毎回照合する。
    #[cfg(windows)]
    pub(crate) fn pty_approved(&self, id: &str) -> Result<bool, String> {
        let packages = self
            .runtime_packages
            .lock()
            .map_err(|error| error.to_string())?;
        let package = packages.get(id).ok_or("実行中の拡張がありません")?;
        if !declares_pty_contract(package) {
            return Ok(false);
        }
        let capability = extensions_entry::PackageCapability {
            id: ContractId::try_new("process.pty").expect("valid capability"),
            version: semver::Version::new(1, 0, 0),
        };
        let directory = self
            .permissions_directory
            .as_ref()
            .ok_or("拡張の状態ディレクトリがありません")?;
        extensions_entry::CapabilityApprovals::new(directory.clone())
            .is_granted(package, &capability)
            .map_err(|error| error.to_string())
    }

    /// 検証済みの導入 package が宣言した能力だけを承認・取消する。
    #[cfg(windows)]
    pub(crate) fn set_pty_approval(
        &self,
        id: &str,
        granted: bool,
        expected_digest: &str,
    ) -> Result<(), String> {
        let id = ExtensionPackageId::try_new(id).map_err(|error| error.to_string())?;
        let capability_id = ContractId::try_new("process.pty").expect("valid capability");
        if !granted {
            return self.revoke_capability(&id, &capability_id);
        }
        let loader = self.loader()?;
        let mut catalog = self.catalog_mut()?;
        let package = loader
            .load_installed(&mut catalog, &id)
            .map_err(|error| error.to_string())?;
        if package.manifest_digest() != expected_digest {
            return Err("Extension changed; review permissions again".into());
        }
        if !declares_pty_contract(&package) {
            return Err("PTY v1 契約が宣言されていません".into());
        }
        let capability = extensions_entry::PackageCapability {
            id: capability_id,
            version: semver::Version::new(1, 0, 0),
        };
        extensions_entry::CapabilityApprovals::new(catalog.root().to_owned())
            .set(&package, &capability, granted)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn revoke_capability(
        &self,
        id: &ExtensionPackageId,
        capability: &ContractId,
    ) -> Result<(), String> {
        let directory = self
            .permissions_directory
            .as_ref()
            .ok_or("拡張の状態ディレクトリがありません")?;
        extensions_entry::CapabilityApprovals::new(directory.clone())
            .revoke_capability(id, capability)
            .map_err(|error| error.to_string())
    }

    #[cfg(windows)]
    pub(crate) fn runtime_ids(&self) -> Vec<String> {
        self.runtime_packages
            .lock()
            .map(|packages| packages.keys().cloned().collect())
            .unwrap_or_default()
    }

    pub(crate) fn capabilities(
        &self,
        id: &str,
    ) -> Result<client_api::HostExtensionCapabilitiesResponse, String> {
        let id = ExtensionPackageId::try_new(id).map_err(|error| error.to_string())?;
        let loader = self.loader()?;
        let mut catalog = self.catalog_mut()?;
        let package = loader
            .inspect_installed(&mut catalog, &id)
            .map_err(|error| error.to_string())?;
        let approvals = extensions_entry::CapabilityApprovals::new(catalog.root().to_owned());
        let mut capabilities = Vec::new();
        for capability in &package.manifest.capabilities {
            #[cfg(windows)]
            let supported = self.supports_pty
                && loader.is_compatible(&package)
                && capability.id.as_str() == "process.pty"
                && capability.version == semver::Version::new(1, 0, 0)
                && declares_pty_contract(&package);
            #[cfg(not(windows))]
            let supported = false;
            capabilities.push(client_api::HostExtensionCapability {
                id: capability.id.as_str().to_owned(),
                version: capability.version.to_string(),
                approved: approvals
                    .is_granted(&package, capability)
                    .map_err(|error| error.to_string())?,
                supported,
            });
        }
        Ok(client_api::HostExtensionCapabilitiesResponse {
            id: id.as_str().to_owned(),
            manifest_digest: package.manifest_digest().to_owned(),
            capabilities,
        })
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
    fn dispatch_contract_is_checked_against_the_startup_package() {
        use sha2::Digest as _;
        use std::io::Write as _;

        let state = tempfile::tempdir().unwrap();
        let mut contracts = HostContracts::current();
        for name in [
            extension_runtime::LIFECYCLE_CONTRACT,
            extension_runtime::DISPATCH_CONTRACT,
        ] {
            contracts.add_version(
                ContractId::try_new(name).unwrap(),
                semver::Version::new(1, 0, 0),
            );
        }
        let mut extensions = HostExtensions::load_with_development(Some(state.path()), true);
        extensions.loader = Loader::Ready(
            ExtensionLoader::new(TrustedKeys::repository().unwrap(), contracts)
                .allow_unsigned_local_development(),
        );
        for (id, dispatch) in [
            ("dev.example.dispatch", true),
            ("dev.example.legacy", false),
        ] {
            let component = b"fixture component";
            let mut requires = vec![serde_json::json!({
                "contract": extension_runtime::LIFECYCLE_CONTRACT, "minimum":"1.0.0", "maximum":"1.0.0"
            })];
            if dispatch {
                requires.push(serde_json::json!({
                    "contract": extension_runtime::DISPATCH_CONTRACT, "minimum":"1.0.0", "maximum":"1.0.0"
                }));
            }
            let manifest = serde_json::json!({
                "schema":1, "kind":"extension", "id":id, "version":"1.0.0", "requires":requires,
                "files":[{"path":"extension.wasm", "sha256":format!("{:x}", sha2::Sha256::digest(component))}]
            });
            let path = state.path().join(format!("{id}.lapis"));
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file("manifest.json", options).unwrap();
            zip.write_all(&serde_json::to_vec(&manifest).unwrap())
                .unwrap();
            zip.start_file("extension.wasm", options).unwrap();
            zip.write_all(component).unwrap();
            zip.finish().unwrap();
            extensions.install(&path, true).unwrap();
        }
        let (components, _, _, _, failures) = extensions.startup_components();
        assert!(failures.is_empty(), "{failures:?}");
        assert_eq!(components.len(), 2);
        assert_eq!(
            extensions.declares_dispatch("dev.example.dispatch"),
            Ok(true)
        );
        assert_eq!(
            extensions.declares_dispatch("dev.example.legacy"),
            Ok(false)
        );
        assert!(extensions.declares_dispatch("dev.example.missing").is_err());
        extensions
            .set_enabled("dev.example.dispatch", false)
            .unwrap();
        assert_eq!(
            extensions.declares_dispatch("dev.example.dispatch"),
            Ok(true)
        );
        extensions.startup_components();
        assert!(
            extensions
                .declares_dispatch("dev.example.dispatch")
                .is_err()
        );
    }

    #[test]
    fn runtime_contributions_require_a_successfully_active_instance() {
        let contribution = StartupContribution {
            payload: client_api::HostExtensionContributions {
                extension_id: "dev.example.extension".into(),
                declaration: extension_ui_contract::StaticContributions {
                    schema: 1,
                    default_locale: "en-US".into(),
                    settings: Vec::new(),
                    themes: Vec::new(),
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
    fn invalid_bundled_package_does_not_hide_a_later_valid_package() {
        use sha2::Digest as _;
        use std::io::Write as _;

        let state = tempfile::tempdir().unwrap();
        let bundled = tempfile::tempdir().unwrap();
        std::fs::write(bundled.path().join("00-bad.lapis"), b"invalid").unwrap();
        let data = b"{}";
        let manifest = serde_json::json!({
            "schema": 1, "kind": "extension", "id": "dev.example.good", "version": "1.0.0",
            "requires": [{"contract": "lapis.extension", "minimum": "1.0.0", "maximum": "1.0.0"}],
            "files": [{"path": "data.json", "sha256": format!("{:x}", sha2::Sha256::digest(data))}]
        });
        let file = std::fs::File::create(bundled.path().join("01-good.lapis")).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        archive.start_file("manifest.json", options).unwrap();
        archive
            .write_all(&serde_json::to_vec(&manifest).unwrap())
            .unwrap();
        archive.start_file("data.json", options).unwrap();
        archive.write_all(data).unwrap();
        archive.finish().unwrap();

        let host = HostExtensions::load_with_development(Some(state.path()), true);
        assert!(host.install_bundled(bundled.path()).is_err());
        let list = host.list().unwrap();
        assert_eq!(list.extensions.len(), 1);
        assert_eq!(list.extensions[0].id, "dev.example.good");
    }

    #[test]
    fn bundled_language_is_loaded_once_and_respects_user_disable() {
        use base64::Engine as _;

        let state = tempfile::tempdir().unwrap();
        let bundled = tempfile::tempdir().unwrap();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let seed = [23u8; 32];
        let seed_file = state.path().join("test-seed");
        std::fs::write(&seed_file, seed).unwrap();
        let keys = state.path().join("extensions");
        std::fs::create_dir_all(&keys).unwrap();
        let public = ed25519_dalek::SigningKey::from_bytes(&seed)
            .verifying_key()
            .to_bytes();
        std::fs::write(keys.join("trusted-keys.json"), serde_json::to_vec(&serde_json::json!({
            "schema": 1, "keys": {"test-publisher": base64::engine::general_purpose::STANDARD.encode(public)}
        })).unwrap()).unwrap();
        let mut command = std::process::Command::new("python");
        command
            .current_dir(&root)
            .args(["-m", "scripts.extensions.package"])
            .arg(root.join("extensions/language-ja"))
            .arg(bundled.path().join("language.lapis"))
            .arg("--seed-file")
            .arg(seed_file)
            .args(["--key-id", "test-publisher"]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        assert!(command.status().unwrap().success());
        let host = HostExtensions::load(Some(state.path()));
        host.install_bundled(bundled.path()).unwrap();
        let (_, contributions, _, _, failures) = host.startup_components();
        assert!(failures.is_empty(), "{failures:?}");
        assert!(contributions.iter().any(|item| {
            item.payload.extension_id == "lapis.language.ja"
                && item
                    .payload
                    .declaration
                    .translations
                    .iter()
                    .any(|translation| {
                        translation.target == "lapis.core" && translation.locale == "ja-JP"
                    })
        }));
        host.set_enabled("lapis.language.ja", false).unwrap();
        host.install_bundled(bundled.path()).unwrap();
        assert!(!host.list().unwrap().extensions[0].enabled);
        assert!(host.startup_components().1.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn pty_requires_approval_and_revocation_reclaims_sessions() {
        use extension_runtime::supervisor::HostCallHandler;
        use std::sync::Arc;

        let state = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("extension.wasm"), b"component").unwrap();
        std::fs::write(source.path().join("extension.json"), serde_json::to_vec(&serde_json::json!({
            "id": "dev.example.pty", "version": "1.0.0",
            "requires": [{"contract": "lapis.extension", "minimum": "1.0.0", "maximum": "1.0.0"},
                         {"contract": "lapis.process.pty", "minimum": "1.0.0", "maximum": "1.0.0"}],
            "capabilities": [{"id": "process.pty", "version": "1.0.0"}],
            "files": ["extension.wasm"]
        })).unwrap()).unwrap();
        let archive = state.path().join("fixture.lapis");
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut command = std::process::Command::new("python");
        command
            .current_dir(root)
            .args(["-m", "scripts.extensions.package"])
            .arg(source.path())
            .arg(&archive)
            .arg("--development");
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
        assert!(command.status().unwrap().success());
        let mut extensions = HostExtensions::load_with_development(Some(state.path()), true);
        let mut contracts = HostContracts::current();
        contracts.add_version(
            ContractId::try_new("lapis.process.pty").unwrap(),
            semver::Version::new(1, 0, 0),
        );
        extensions.loader = Loader::Ready(
            ExtensionLoader::new(TrustedKeys::repository().unwrap(), contracts)
                .allow_unsigned_local_development(),
        );
        let extensions = Arc::new(extensions);
        extensions.install(&archive, true).unwrap();
        let mut package = {
            let mut catalog = extensions.catalog_mut().unwrap();
            extensions
                .loader()
                .unwrap()
                .load_installed(
                    &mut catalog,
                    &ExtensionPackageId::try_new("dev.example.pty").unwrap(),
                )
                .unwrap()
        };
        let required = package
            .manifest
            .requires
            .iter()
            .position(|item| item.contract.as_str() == "lapis.process.pty")
            .unwrap();
        let contract = package.manifest.requires.remove(required);
        assert!(!declares_pty_contract(&package));
        package.manifest.requires.insert(required, contract);
        extensions
            .runtime_packages
            .lock()
            .unwrap()
            .insert("dev.example.pty".into(), package);
        let digest = extensions
            .capabilities("dev.example.pty")
            .unwrap()
            .manifest_digest;
        let broker = crate::extension_pty::PtyBroker::new(extensions);
        let spawn = serde_json::json!({"program":"cmd.exe", "args":["/D", "/C", "ping -n 60 127.0.0.1 >nul"],
            "env":{}, "cwd": state.path().to_string_lossy(), "size":{"columns":80,"rows":24}});
        assert!(
            broker
                .call("dev.example.pty", "pty.v1.spawn", spawn.clone())
                .is_err()
        );
        assert!(
            broker
                .set_approval("dev.example.pty", true, &"a".repeat(64))
                .is_err()
        );
        broker
            .set_approval("dev.example.pty", true, &digest)
            .unwrap();
        let id = broker
            .call("dev.example.pty", "pty.v1.spawn", spawn)
            .unwrap();
        broker
            .call(
                "dev.example.pty",
                "pty.v1.attach",
                serde_json::json!({"id":id}),
            )
            .unwrap();
        broker
            .set_approval("dev.example.pty", false, &digest)
            .unwrap();
        assert!(
            broker
                .call(
                    "dev.example.pty",
                    "pty.v1.attach",
                    serde_json::json!({"id":id})
                )
                .is_err()
        );
        assert!(
            broker
                .set_approval("dev.example.pty", true, &"a".repeat(64))
                .is_err()
        );
        broker
            .set_approval("dev.example.pty", true, &digest)
            .unwrap();
        assert!(
            broker
                .call(
                    "dev.example.pty",
                    "pty.v1.attach",
                    serde_json::json!({"id":id})
                )
                .is_err()
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "Wasm fixture と拡張ホストの事前ビルドが必要"]
    fn approved_wasm_uses_host_pty_and_shutdown_reclaims_it() {
        use extension_runtime::supervisor::{ExtensionSupervisor, SupervisorStatus};
        use std::{
            sync::Arc,
            time::{Duration, Instant},
        };

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let component = std::fs::read(
            root.join("target/pty-extension/wasm32-wasip2/debug/example_pty_extension.wasm"),
        )
        .unwrap();
        let executable = root.join("target/debug/lapis-extension-host.exe");
        let state = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("extension.wasm"), component).unwrap();
        std::fs::write(source.path().join("extension.json"), serde_json::to_vec(&serde_json::json!({
            "id":"dev.example.pty", "version":"1.0.0",
            "requires":[{"contract":"lapis.extension", "minimum":"1.0.0", "maximum":"1.0.0"},
                        {"contract":"lapis.extension.lifecycle", "minimum":"1.0.0", "maximum":"1.0.0"},
                        {"contract":"lapis.process.pty", "minimum":"1.0.0", "maximum":"1.0.0"}],
            "capabilities":[{"id":"process.pty", "version":"1.0.0"}], "files":["extension.wasm"]
        })).unwrap()).unwrap();
        let archive = state.path().join("fixture.lapis");
        let mut command = std::process::Command::new("python");
        command
            .current_dir(&root)
            .args(["-m", "scripts.extensions.package"])
            .arg(source.path())
            .arg(&archive)
            .arg("--development");
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
        assert!(command.status().unwrap().success());

        let mut extensions = HostExtensions::load_with_development(Some(state.path()), true);
        let mut contracts = HostContracts::current();
        for id in ["lapis.extension.lifecycle", "lapis.process.pty"] {
            contracts.add_version(
                ContractId::try_new(id).unwrap(),
                semver::Version::new(1, 0, 0),
            );
        }
        extensions.loader = Loader::Ready(
            ExtensionLoader::new(TrustedKeys::repository().unwrap(), contracts)
                .allow_unsigned_local_development(),
        );
        extensions.install(&archive, true).unwrap();
        let (components, _, _, _, failures) = extensions.startup_components();
        assert!(failures.is_empty(), "{failures:?}");
        let digest = extensions
            .capabilities("dev.example.pty")
            .unwrap()
            .manifest_digest;
        let broker = Arc::new(crate::extension_pty::PtyBroker::new(Arc::new(extensions)));
        let wait_for_running = |supervisor: &ExtensionSupervisor| {
            let deadline = Instant::now() + Duration::from_secs(20);
            loop {
                match supervisor.status() {
                    SupervisorStatus::Running { active, failed, .. } => return (active, failed),
                    SupervisorStatus::Stopped { reason } => panic!("{reason}"),
                    SupervisorStatus::Starting => {}
                }
                assert!(Instant::now() < deadline, "extension host did not start");
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        let denied = ExtensionSupervisor::start_with_handler(
            executable.clone(),
            components.clone(),
            Some(broker.clone()),
        );
        let (active, failed) = wait_for_running(&denied);
        assert!(active.is_empty());
        assert!(failed.contains_key("dev.example.pty"));
        assert_eq!(broker.session_count("dev.example.pty"), 0);
        drop(denied);
        assert!(
            broker
                .set_approval("dev.example.pty", true, &"a".repeat(64))
                .is_err()
        );
        broker
            .set_approval("dev.example.pty", true, &digest)
            .unwrap();
        let supervisor =
            ExtensionSupervisor::start_with_handler(executable, components, Some(broker.clone()));
        let (active, failed) = wait_for_running(&supervisor);
        assert!(failed.is_empty(), "{failed:?}");
        assert_eq!(active, ["dev.example.pty"]);
        assert_eq!(broker.session_count("dev.example.pty"), 1);
        drop(supervisor);
        assert_eq!(broker.session_count("dev.example.pty"), 0);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "Terminal component と拡張ホストの事前ビルドが必要"]
    fn terminal_component_uses_approved_host_pty() {
        use extension_runtime::supervisor::{ExtensionSupervisor, SupervisorStatus};
        use std::{
            os::windows::process::CommandExt,
            sync::Arc,
            time::{Duration, Instant},
        };

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let state = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        std::fs::copy(
            root.join(
                "target/terminal-component/wasm32-wasip2/debug/lapis_terminal_component.wasm",
            ),
            source.path().join("extension.wasm"),
        )
        .unwrap();
        std::fs::copy(
            root.join("extensions/terminal/component/extension.json"),
            source.path().join("extension.json"),
        )
        .unwrap();
        let archive = state.path().join("terminal.lapis");
        assert!(
            std::process::Command::new("python")
                .current_dir(&root)
                .args(["-m", "scripts.extensions.package"])
                .arg(source.path())
                .arg(&archive)
                .arg("--development")
                .creation_flags(0x08000000)
                .status()
                .unwrap()
                .success()
        );
        let mut extensions = HostExtensions::load_with_development(Some(state.path()), true);
        let mut contracts = HostContracts::current();
        for id in [
            extension_runtime::LIFECYCLE_CONTRACT,
            extension_runtime::DISPATCH_CONTRACT,
            "lapis.process.pty",
        ] {
            contracts.add_version(
                ContractId::try_new(id).unwrap(),
                semver::Version::new(1, 0, 0),
            );
        }
        extensions.loader = Loader::Ready(
            ExtensionLoader::new(TrustedKeys::repository().unwrap(), contracts)
                .allow_unsigned_local_development(),
        );
        extensions.install(&archive, true).unwrap();
        let (components, _, _, _, failures) = extensions.startup_components();
        assert!(failures.is_empty(), "{failures:?}");
        let digest = extensions
            .capabilities("lapis.terminal")
            .unwrap()
            .manifest_digest;
        let broker = Arc::new(crate::extension_pty::PtyBroker::new(Arc::new(extensions)));
        let supervisor = ExtensionSupervisor::start_with_handler(
            root.join("target/debug/lapis-extension-host.exe"),
            components,
            Some(broker.clone()),
        );
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match supervisor.status() {
                SupervisorStatus::Running { active, failed, .. } => {
                    assert!(failed.is_empty(), "{failed:?}");
                    assert_eq!(active, ["lapis.terminal"]);
                    break;
                }
                SupervisorStatus::Stopped { reason } => panic!("{reason}"),
                SupervisorStatus::Starting => {}
            }
            assert!(Instant::now() < deadline, "extension host did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
        let start = serde_json::json!({
            "program":"cmd.exe", "arguments":["/D","/C","echo LapisTerminal & ping -n 30 127.0.0.1 >nul"],
            "working_directory":state.path().to_string_lossy(), "columns":80, "rows":24
        }).to_string();
        assert!(
            supervisor
                .dispatch("lapis.terminal", "start", &start)
                .is_err()
        );
        assert_eq!(broker.session_count("lapis.terminal"), 0);
        broker
            .set_approval("lapis.terminal", true, &digest)
            .unwrap();
        let opened: serde_json::Value = serde_json::from_str(
            &supervisor
                .dispatch("lapis.terminal", "start", &start)
                .unwrap(),
        )
        .unwrap();
        let id = opened["id"].as_str().unwrap();
        assert_eq!(broker.session_count("lapis.terminal"), 1);
        let mut offset = 0u64;
        let mut output = Vec::new();
        let mut answered_cursor_query = false;
        let deadline = Instant::now() + Duration::from_secs(10);
        while !output
            .windows(b"LapisTerminal".len())
            .any(|part| part == b"LapisTerminal")
        {
            assert!(
                Instant::now() < deadline,
                "terminal produced no output: {output:?}"
            );
            let request = serde_json::json!({"id": id, "offset": offset, "max_bytes": 8192, "timeout_ms": 500});
            let response: serde_json::Value = serde_json::from_str(
                &supervisor
                    .dispatch("lapis.terminal", "read", &request.to_string())
                    .unwrap(),
            )
            .unwrap();
            offset = response["next"].as_u64().unwrap();
            output.extend(
                response["bytes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|byte| byte.as_u64().unwrap() as u8),
            );
            if !answered_cursor_query && output.windows(4).any(|part| part == b"\x1b[6n") {
                supervisor
                    .dispatch(
                        "lapis.terminal",
                        "write",
                        &serde_json::json!({
                            "id":id,"bytes": [27, 91, 49, 59, 49, 82]
                        })
                        .to_string(),
                    )
                    .unwrap();
                answered_cursor_query = true;
            }
        }
        supervisor
            .dispatch(
                "lapis.terminal",
                "resize",
                &serde_json::json!({"id":id,"columns":100,"rows":30}).to_string(),
            )
            .unwrap();
        supervisor
            .dispatch(
                "lapis.terminal",
                "close",
                &serde_json::json!({"id":id}).to_string(),
            )
            .unwrap();
        let second: serde_json::Value = serde_json::from_str(
            &supervisor
                .dispatch("lapis.terminal", "start", &start)
                .unwrap(),
        )
        .unwrap();
        let second_id = second["id"].as_str().unwrap();
        broker
            .set_approval("lapis.terminal", false, &digest)
            .unwrap();
        assert_eq!(broker.session_count("lapis.terminal"), 0);
        assert!(
            supervisor
                .dispatch(
                    "lapis.terminal",
                    "read",
                    &serde_json::json!({"id":second_id,"offset":0,"max_bytes":64,"timeout_ms":0})
                        .to_string(),
                )
                .is_err()
        );
        drop(supervisor);
        assert_eq!(broker.session_count("lapis.terminal"), 0);
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
