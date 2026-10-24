mod layout;
pub(crate) mod main;
mod panel;
pub(crate) use panel::panel_size;
pub(crate) mod panel_transition;
mod state;
mod tab_state;
pub(crate) mod window_group;

pub(crate) use layout::DockLayout;
pub(crate) use panel::PanelHost;
pub(crate) use state::{
    HeaderMenuSection, PanelSpanTransition, ResizeMode, ResizeTarget, ShellState,
};
pub(crate) use tab_state::{DraggedPanelTab, PanelTab, ViewInstanceId};
