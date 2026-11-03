//! Browser向けClientの起動・通信境界。

#[cfg(target_arch = "wasm32")]
pub mod connection;

#[cfg(all(target_arch = "wasm32", feature = "bridge-smoke"))]
mod bridge_smoke;
