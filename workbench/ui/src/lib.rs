mod app;
#[cfg(target_family = "wasm")]
extern crate gpui_preview as gpui;
#[cfg_attr(not(target_family = "wasm"), allow(dead_code))]
mod bundled_font;
mod client_adapter;
mod client_worker;
pub(crate) use workbench_shell::commands;
mod desktop;
#[cfg(all(debug_assertions, not(target_family = "wasm")))]
#[path = "../../../tools/devtools/mod.rs"]
mod devtools;
mod entry;
#[cfg(not(target_family = "wasm"))]
mod extension_loading;
pub use workbench_shell::extension_ui;
mod features;
pub(crate) use ui_kit::gpui_compat;
#[cfg(all(feature = "hot-reload", debug_assertions))]
mod hot_reload;
pub mod keymap;
pub use ui_kit as kit;
pub use ui_kit::localization;
pub use ui_kit::notification;
mod root;
pub(crate) use ui_kit::time;
pub(crate) use ui_kit::timer;
pub(crate) use workbench_shell as shell;
pub mod web_bridge;
pub(crate) use workbench_shell::workbench;

pub(crate) use desktop::window_host;
pub(crate) use ui_kit::components;
pub use ui_kit::{theme, tokens};

pub use app::{ConnectionIdentity, DesktopServices, InitialView, UpdaterHandle, WorkspaceOption};
#[cfg(not(target_family = "wasm"))]
pub use app::{
    desktop_application, init_tracing, initialize, on_last_window_closed, open_desktop_window,
};
#[cfg(target_family = "wasm")]
pub use entry::web_fonts;
pub use entry::{application_assets, boot};
