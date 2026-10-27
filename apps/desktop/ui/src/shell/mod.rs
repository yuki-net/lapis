mod dock_geometry;
mod layout;
pub(crate) mod main;
mod panel;
mod panel_layout;
pub(crate) mod panel_transition;
mod state;
mod tab_state;
pub(crate) mod window_group;
mod zone;
mod zone_policy;

pub(crate) use layout::{DockLayout, PanelDrop};
pub(crate) use panel::PanelHost;
pub(crate) use panel_layout::PanelEdge;
pub(crate) use state::{
    HeaderMenuSection, PanelSpanTransition, PickerTarget, ResizeMode, ResizeTarget, ShellState,
};
pub(crate) use tab_state::{DraggedPanelTab, PanelId, PanelTab, ViewInstanceId};
