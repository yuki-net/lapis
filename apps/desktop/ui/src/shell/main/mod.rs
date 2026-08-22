use super::*;
pub(crate) use crate::tokens;
use crate::{
    extension_ui::PanelPosition,
    shell::{DraggedPanelTab, PanelHost, PanelTab, ResizeTarget},
};

mod actions;
mod command_palette;
mod content;
mod footer;
mod header;
mod layout;
mod overlays;
mod panel_frame;
mod panel_tabs;
mod render;
mod resize;
mod tool_picker;

const fn panel_key(position: PanelPosition) -> u32 {
    match position {
        PanelPosition::Left => 1,
        PanelPosition::Main => 2,
        PanelPosition::Bottom => 3,
        PanelPosition::Right => 4,
    }
}
