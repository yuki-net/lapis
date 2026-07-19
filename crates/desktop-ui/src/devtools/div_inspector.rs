use gpui::{Div, DivInspectorState, InspectorElementId, div, prelude::*};

use crate::theme;

use super::style_view::{detail_row, render_style};

pub(super) fn render_div_inspector(_id: InspectorElementId, state: &DivInspectorState) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(section_title("Div content"))
        .child(detail_row(
            "content width",
            pixels(state.content_size.width),
        ))
        .child(detail_row(
            "content height",
            pixels(state.content_size.height),
        ))
        .child(render_style(&state.base_style))
}

fn section_title(title: &str) -> Div {
    div()
        .pt_2()
        .pb_1()
        .text_color(theme::accent())
        .child(title.to_owned())
}

fn pixels(value: gpui::Pixels) -> String {
    format!("{:.1}px", f32::from(value))
}
