mod app;
#[cfg(target_family = "wasm")]
extern crate gpui_preview as gpui;
mod client_adapter;
mod client_worker;
mod commands;
mod desktop;
#[cfg(all(debug_assertions, not(target_family = "wasm")))]
mod devtools;
pub mod extension_ui;
mod features;
mod gpui_compat;
#[cfg(all(feature = "hot-reload", debug_assertions))]
mod hot_reload;
pub mod keymap;
pub mod kit;
pub mod localization;
pub mod notification;
mod root;
mod shell;
mod timer;
#[cfg(target_family = "wasm")]
mod web;
mod workbench;

pub(crate) use desktop::window_host;
pub(crate) use kit::components;
pub use kit::{theme, tokens};

#[cfg(target_family = "wasm")]
pub use web::{connection_info, is_ready, start};

/// wasmにはstdの時刻APIが無いため、ブラウザーではweb-timeを使う。
pub(crate) mod time {
    pub(crate) use web_time::{Duration, Instant, SystemTime};
}

pub use app::{DesktopServices, InitialView};
#[cfg(not(target_family = "wasm"))]
pub use desktop::run;
#[cfg(target_family = "wasm")]
pub use web::run_web;
