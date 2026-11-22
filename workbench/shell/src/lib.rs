//! Workbench shell: layout, panel state and window presentation shared by clients.
//!
//! The shell owns panel/layout state and the GPUI presentation around it. Feature
//! implementations (views and their domain binding) live in `desktop-ui` and are
//! supplied through the feature registry.

#![allow(special_module_name)]

#[cfg(target_family = "wasm")]
extern crate gpui_preview as gpui;

pub mod actions;
pub mod chrome;
pub mod command_search;
pub mod commands;
mod dock_geometry;
pub mod drag_preview;
pub mod extension_ui;
pub mod file_display;
pub mod ids;
mod layout;
#[allow(special_module_name)]
pub mod main;
mod panel;
mod panel_layout;
pub mod panel_transition;
pub mod screen;
mod state;
mod tab_state;
#[cfg(test)]
mod test_support;
pub mod view_state;
pub mod window_group;
pub mod workbench;
mod zone;
mod zone_policy;

pub use commands::{EXTRA_COMMANDS, InvokeCommand};
pub use layout::{DockLayout, PanelDrop};
pub use panel::PanelHost;
pub use panel_layout::PanelEdge;
pub use state::{
    HeaderMenuSection, PanelSpanTransition, PickerTarget, ResizeMode, ResizeTarget, ShellState,
};
pub use tab_state::{DraggedPanelTab, PanelId, PanelTab, ViewInstanceId};

// The shell renders with the shared UI kit. Re-export the paths that were previously
// available through `desktop-ui` so moved shell code keeps its imports.
pub use ui_kit as kit;
pub use ui_kit::{components, gpui_compat, localization, notification, theme, time, timer, tokens};

/// Workspace selection candidate shown in the connection menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceOption {
    pub id: String,
    pub name: String,
}

/// Display data for the connected Host shown by the shell.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConnectionIdentity {
    pub host_name: Option<String>,
    pub host_id: Option<String>,
    /// Present when the UI owns the Host-side workspace name (for example Web).
    pub workspace_name: Option<String>,
    pub workspace_id: Option<String>,
    /// Whether this connection can switch workspaces.
    pub can_switch: bool,
    /// Switch candidates. Kept as returned by `workspace.list`.
    pub workspaces: Vec<WorkspaceOption>,
}

/// Shortcut label lookup supplied by the client keymap implementation.
pub type ShortcutLabelFn = fn(&gpui::App, &extension_ui_contract::CommandId) -> String;

/// Keymap adapter registered by `desktop-ui`. The shell must not depend on the
/// keymap crate because the keymap binds desktop-only actions.
#[derive(Clone, Copy, Default)]
pub struct ShortcutLabels(pub Option<ShortcutLabelFn>);

impl gpui::Global for ShortcutLabels {}

/// Returns the display label for a command, or an empty string when no keymap is
/// registered (for example in shell unit tests).
pub fn shortcut_label(cx: &gpui::App, command: &extension_ui_contract::CommandId) -> String {
    cx.try_global::<ShortcutLabels>()
        .and_then(|labels| labels.0)
        .map(|label| label(cx, command))
        .unwrap_or_default()
}

/// DevTool toggle supplied by the native client. The shell must not depend on
/// `tools/devtools` directly because devtools inspects the desktop root.
pub type DevtoolsToggleFn = fn(&mut gpui::Window, &mut gpui::App) -> Result<bool, String>;

#[derive(Clone, Copy, Default)]
pub struct DevtoolsToggle(pub Option<DevtoolsToggleFn>);

impl gpui::Global for DevtoolsToggle {}

pub fn toggle_devtools(window: &mut gpui::Window, cx: &mut gpui::App) -> Result<bool, String> {
    cx.try_global::<DevtoolsToggle>()
        .and_then(|toggle| toggle.0)
        .map(|toggle| toggle(window, cx))
        .unwrap_or(Ok(false))
}

/// 画面全体の物理座標。異なる DPI の Window 間で論理座標を混ぜない。
#[derive(Clone, Copy, Debug)]
#[cfg_attr(target_family = "wasm", allow(dead_code))]
pub struct ScreenPoint {
    pub x: i32,
    pub y: i32,
}
