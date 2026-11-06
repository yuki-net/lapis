//! ブラウザー向けClientの通信・回帰検証用。本番のWeb UIは `desktop-ui` のWasm entryから起動する。
//!
//! 本番経路と混同しないよう、このcrateはテスト・検証用途に限定する。

#[cfg(target_arch = "wasm32")]
pub mod connection;

#[cfg(target_arch = "wasm32")]
mod session;

#[cfg(target_arch = "wasm32")]
pub use session::WebSession;

#[cfg(all(target_arch = "wasm32", feature = "bridge-smoke"))]
mod bridge_smoke;
