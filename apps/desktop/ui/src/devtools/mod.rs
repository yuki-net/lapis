mod div_inspector;
mod inspector_view;
mod style_view;

#[cfg(test)]
mod tests;

use gpui::{App, DivInspectorState};

pub(crate) fn init(cx: &mut App) {
    cx.set_inspector_renderer(Box::new(inspector_view::render_inspector));
    cx.register_inspector_element(|id, state: &DivInspectorState, _window, _cx| {
        div_inspector::render_div_inspector(id, state)
    });
}
