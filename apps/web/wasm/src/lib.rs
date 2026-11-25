//! ブラウザー向けの Web entry。
//!
//! 通信・回帰検証用の `WebSession` と、本番の UI を起動する wasm entry を持つ。

#[cfg(target_arch = "wasm32")]
pub mod connection;
#[cfg(target_arch = "wasm32")]
extern crate gpui_preview as gpui;
#[cfg(target_arch = "wasm32")]
mod entry;
#[cfg(any(target_arch = "wasm32", test))]
mod prototype;
#[cfg(target_arch = "wasm32")]
mod session;
#[cfg(target_arch = "wasm32")]
mod startup;

#[cfg(target_arch = "wasm32")]
pub use entry::{connection_info, is_ready, start};
#[cfg(target_arch = "wasm32")]
pub use session::WebSession;
