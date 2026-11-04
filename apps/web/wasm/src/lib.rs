//! Browser向けClientの通信境界。GPUI UIの起動は lapis-desktop-ui 側が持つ。

#[cfg(target_arch = "wasm32")]
pub mod connection;

#[cfg(target_arch = "wasm32")]
mod session;

#[cfg(target_arch = "wasm32")]
pub use session::WebSession;

#[cfg(all(target_arch = "wasm32", feature = "bridge-smoke"))]
mod bridge_smoke;
