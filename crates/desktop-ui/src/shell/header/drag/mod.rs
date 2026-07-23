//! Header drag behavior, selected for the current target platform.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
pub(crate) use linux::apply_drag_region;
#[cfg(target_os = "macos")]
pub(crate) use macos::apply_drag_region;
#[cfg(target_os = "windows")]
pub(crate) use windows::apply_drag_region;

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub(crate) fn apply_drag_region(region: gpui::Div) -> gpui::Div {
    region
}
