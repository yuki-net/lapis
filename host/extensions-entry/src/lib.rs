//! 拡張の入口。導入済み拡張の目録と状態、package・データの置き場所を持つ。
//!
//! `.lapis` package の読み込み・互換照合・署名検証・Wasm 実行はこの crate の
//! 対象外。ここは「どの拡張が入っていて、有効か」と「その実体をどこへ置くか」
//! だけを扱い、状態の宣言として Client へ見せる。

mod catalog;
mod id;
mod state;
mod version;

pub use catalog::{
    CATALOG_SCHEMA, CatalogDocument, ExtensionCatalog, ExtensionCatalogError, ExtensionEntry,
    Registration,
};
pub use id::{ExtensionPackageId, InvalidExtensionPackageId};
pub use state::ExtensionState;
pub use version::{ExtensionVersion, InvalidExtensionVersion};
