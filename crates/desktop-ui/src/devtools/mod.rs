mod div_inspector;
mod inspector_controller;
mod inspector_window;
mod style_view;
mod tree_view;

use gpui::{App, DivInspectorState, Window};

pub(crate) fn init(cx: &mut App) {
    inspector_controller::init(cx);
    cx.set_inspector_renderer(Box::new(inspector_window::render_inspector));
    cx.register_inspector_element(|id, state: &DivInspectorState, _window, _cx| {
        div_inspector::render_div_inspector(id, state)
    });
}

pub(crate) fn toggle_inspector(window: &mut Window, cx: &mut App) -> Result<bool, String> {
    inspector_controller::InspectorController::toggle(window, cx)
}
