use super::super::*;

/// Requests a native window move from the active Wayland or X11 compositor.
pub(crate) fn apply_drag_region(region: gpui::Div) -> gpui::Stateful<gpui::Div> {
    region.on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
}
