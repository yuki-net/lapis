//! Desktop（ネイティブ window）向けの shell 部品と起動処理。

#[cfg(not(target_family = "wasm"))]
mod window;
pub(crate) mod window_host;
mod window_options;

#[cfg(not(target_family = "wasm"))]
pub use window::run;
pub(crate) use window_options::window_options;
