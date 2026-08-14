use super::super::*;

/// Delegates header dragging to Windows non-client hit testing via GPUI.
pub(crate) fn apply_drag_region(region: gpui::Div) -> gpui::Div {
    region.window_control_area(WindowControlArea::Drag)
}
