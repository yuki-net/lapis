mod app;
#[cfg(target_family = "wasm")]
extern crate gpui_preview as gpui;
mod client_adapter;
mod client_worker;
mod commands;
mod components;
#[cfg(all(debug_assertions, not(target_family = "wasm")))]
mod devtools;
pub mod extension_ui;
mod features;
mod gpui_compat;
#[cfg(all(feature = "hot-reload", debug_assertions))]
mod hot_reload;
pub mod keymap;
pub mod localization;
pub mod notification;
mod root;
mod shell;
pub mod theme;
mod timer;
pub mod tokens;
mod window_host;
mod workbench;

/// wasmにはstdの時刻APIが無いため、ブラウザーではweb-timeを使う。
pub(crate) mod time {
    pub(crate) use web_time::{Duration, Instant, SystemTime};
}

#[cfg(not(target_family = "wasm"))]
pub use app::run;
#[cfg(target_family = "wasm")]
pub use app::run_web;
pub use app::{DesktopServices, InitialView};
