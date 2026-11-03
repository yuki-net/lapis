//! Browser向けClientの起動・通信境界。

#[cfg(target_arch = "wasm32")]
pub mod connection;

#[cfg(target_arch = "wasm32")]
mod session;

#[cfg(target_arch = "wasm32")]
pub use session::WebSession;

#[cfg(all(target_arch = "wasm32", feature = "bridge-smoke"))]
mod bridge_smoke;
