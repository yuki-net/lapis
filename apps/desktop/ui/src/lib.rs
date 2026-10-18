mod app;
mod components;
#[cfg(all(debug_assertions, not(target_family = "wasm")))]
mod devtools;
pub mod extension_ui;
mod features;
mod gpui_compat;
pub mod keymap;
pub mod localization;
mod shell;
pub mod theme;
pub mod tokens;

#[cfg(not(target_family = "wasm"))]
pub use app::run;
#[cfg(target_family = "wasm")]
pub use app::run_web;
pub use app::{DesktopServices, InitialView};
#[cfg(target_family = "wasm")]
extern crate gpui_preview as gpui;
