//! 拡張の入口。導入済み拡張の目録と状態、package・データの置き場所を持つ。
//!
//! ここが扱うのは「どの拡張が入っていて、有効か、非互換か」と「その実体をどこへ置くか」、
//! そして `.lapis` package（zip + manifest）の読み込みである。読み込みでは manifest が
//! 要求する契約と版を Host の提供値と照合し、リリース署名と同じ Ed25519 の規則で署名を
//! 検証する。Wasm ランタイム上での実行はこの crate の対象外（将来段階）。

mod catalog;
mod compatibility;
mod declaration;
mod id;
mod journal;
mod package;
mod signature;
mod state;
mod version;

pub use catalog::{
    CATALOG_SCHEMA, CatalogDocument, ExtensionCatalog, ExtensionCatalogError, ExtensionEntry,
    Registration,
};
pub use compatibility::{
    ContractId, ContractRequirement, EXTENSION_CONTRACT, HOST_EXTENSION_CONTRACT_VERSION,
    HostContracts, Incompatibility, InvalidContractId, InvalidContractRequirement,
};
pub use declaration::{ExtensionDeclaration, ExtensionDeclarations};
pub use id::{ExtensionPackageId, InvalidExtensionPackageId};
pub use journal::{JOURNAL_SCHEMA, PendingInstall};
pub use package::{
    ExtensionLoader, InstallOutcome, MANIFEST_NAME, PACKAGE_SCHEMA, PackageError, PackageFile,
    PackageKind, PackageManifest,
};

/// Host の起動時に、中断した導入・除去を完了または取り消す。
///
/// 目録と `installed/<id>/` の書き込みの間で Host が終了しても、次の起動で
/// 食い違いを残さない。目録を公開する前に呼ぶ。
pub fn recover_pending_installs(
    catalog: &mut ExtensionCatalog,
) -> Result<usize, ExtensionCatalogError> {
    journal::recover(catalog)
}
pub use signature::{SignatureError, TrustedKeys};
pub use state::ExtensionState;
pub use version::{ExtensionVersion, InvalidExtensionVersion};
