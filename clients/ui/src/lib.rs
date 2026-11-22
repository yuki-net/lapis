mod app;
#[cfg(target_family = "wasm")]
extern crate gpui_preview as gpui;
#[cfg_attr(not(target_family = "wasm"), allow(dead_code))]
mod bundled_font;
mod client_adapter;
mod client_worker;
mod commands;
mod desktop;
#[cfg(all(debug_assertions, not(target_family = "wasm")))]
mod devtools;
mod entry;
#[cfg(not(target_family = "wasm"))]
mod extension_loading;
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
pub mod web_bridge;
mod workbench;

pub(crate) use desktop::window_host;
pub(crate) use kit::components;
pub use kit::{theme, tokens};

/// wasmにはstdの時刻APIが無いため、ブラウザーではweb-timeを使う。
pub(crate) mod time {
    pub(crate) use web_time::{Duration, Instant, SystemTime};
}

pub use app::{ConnectionIdentity, DesktopServices, InitialView, UpdaterHandle, WorkspaceOption};
#[cfg(not(target_family = "wasm"))]
pub use app::{
    desktop_application, init_tracing, initialize, on_last_window_closed, open_desktop_window,
};
#[cfg(target_family = "wasm")]
pub use entry::web_fonts;
pub use entry::{application_assets, boot};
