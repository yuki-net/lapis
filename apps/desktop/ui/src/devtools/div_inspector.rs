use gpui::{Div, DivInspectorState, InspectorElementId, div, prelude::*, px, rgb, rgba};
use serde_json::Value;

use super::style_view::{detail_row, render_style};
use crate::theme;

pub(super) fn render_div_inspector(_id: InspectorElementId, state: &DivInspectorState) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(section_title("Box Model"))
        .child(render_box_model(state))
        .child(section_title("Div Content"))
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct BoxModelEdges {
    pub top: String,
    pub right: String,
    pub bottom: String,
    pub left: String,
}

fn format_num(n: &serde_json::Number) -> String {
    if let Some(f) = n.as_f64() {
        if f.fract() == 0.0 {
            format!("{:.0}", f)
        } else {
            format!("{f}")
        }
    } else {
        format!("{n}")
    }
}

pub(super) fn extract_edges(value: &Value, field: &str) -> BoxModelEdges {
    let obj = value.get(field);
    let get_side = |side: &str| -> String {
        let side_val = obj.and_then(|o| o.get(side));
        match side_val {
            Some(Value::Number(n)) => format_num(n),
            Some(Value::Object(map)) => {
                if let Some(Value::Number(n)) = map.get("Pixels") {
                    format_num(n)
                } else if let Some(Value::Number(n)) = map.get("Definite") {
                    format_num(n)
                } else {
                    "-".to_owned()
                }
            }
            Some(Value::String(s)) => s.clone(),
            _ => "-".to_owned(),
        }
    };
    BoxModelEdges {
        top: get_side("top"),
        right: get_side("right"),
        bottom: get_side("bottom"),
        left: get_side("left"),
    }
}

pub(super) fn render_box_model(state: &DivInspectorState) -> Div {
    let style_value = serde_json::to_value(&state.base_style).unwrap_or(Value::Null);
    let margin = extract_edges(&style_value, "margin");
    let border = extract_edges(&style_value, "border_widths");
    let padding = extract_edges(&style_value, "padding");
    let content_w = f32::from(state.content_size.width);
    let content_h = f32::from(state.content_size.height);

    div()
        .w_full()
        .p_2()
        .rounded(px(6.0))
        .border_1()
        .border_color(theme::colors().border)
        .bg(theme::colors().island)
        .flex()
        .flex_col()
        .items_center()
        .child(
            div()
                .w_full()
                .p_2()
                .rounded(px(4.0))
                .border_1()
                .border_color(rgba(0xf6b26bcc))
                .bg(rgba(0xf6b26b33))
                .flex()
                .flex_col()
                .items_center()
                .gap_1()
                .child(box_layer_header("margin", &margin.top, rgb(0xf6b26b)))
                .child(
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(side_value(&margin.left, rgb(0xf6b26b)))
                        .child(
                            div()
                                .flex_1()
                                .p_2()
                                .rounded(px(4.0))
                                .border_1()
                                .border_color(rgba(0xffe599cc))
                                .bg(rgba(0xffe59933))
                                .flex()
                                .flex_col()
                                .items_center()
                                .gap_1()
                                .child(box_layer_header("border", &border.top, rgb(0xffe599)))
                                .child(
                                    div()
                                        .w_full()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .gap_2()
                                        .child(side_value(&border.left, rgb(0xffe599)))
                                        .child(
                                            div()
                                                .flex_1()
                                                .p_2()
                                                .rounded(px(4.0))
                                                .border_1()
                                                .border_color(rgba(0x93c47dcc))
                                                .bg(rgba(0x93c47d33))
                                                .flex()
                                                .flex_col()
                                                .items_center()
                                                .gap_1()
                                                .child(box_layer_header("padding", &padding.top, rgb(0x93c47d)))
                                                .child(
                                                    div()
                                                        .w_full()
                                                        .flex()
                                                        .items_center()
                                                        .justify_between()
                                                        .gap_2()
                                                        .child(side_value(&padding.left, rgb(0x93c47d)))
                                                        .child(
                                                            div()
                                                                .flex_1()
                                                                .px_3()
                                                                .py_2()
                                                                .rounded(px(3.0))
                                                                .border_1()
                                                                .border_color(rgba(0x6fa8dccc))
                                                                .bg(rgba(0x6fa8dc4d))
                                                                .flex()
                                                                .items_center()
                                                                .justify_center()
                                                                .child(
                                                                    div()
                                                                        .text_size(px(11.0))
                                                                        .font_weight(gpui::FontWeight::BOLD)
                                                                        .text_color(rgb(0x6fa8dc))
                                                                        .child(format!("{content_w:.1} × {content_h:.1}")),
                                                                ),
                                                        )
                                                        .child(side_value(&padding.right, rgb(0x93c47d))),
                                                )
                                                .child(box_layer_footer(&padding.bottom, rgb(0x93c47d))),
                                        )
                                        .child(side_value(&border.right, rgb(0xffe599))),
                                )
                                .child(box_layer_footer(&border.bottom, rgb(0xffe599))),
                        )
                        .child(side_value(&margin.right, rgb(0xf6b26b))),
                )
                .child(box_layer_footer(&margin.bottom, rgb(0xf6b26b))),
        )
}

fn box_layer_header(name: &'static str, top_val: &str, color: gpui::Rgba) -> Div {
    div()
        .w_full()
        .flex()
        .items_center()
        .justify_between()
        .px_1()
        .child(
            div()
                .text_size(px(9.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(color)
                .child(name),
        )
        .child(
            div()
                .text_size(px(10.0))
                .font_family("Cascadia Mono")
                .text_color(color)
                .child(top_val.to_owned()),
        )
        .child(div().w(px(20.0)))
}

fn box_layer_footer(bottom_val: &str, color: gpui::Rgba) -> Div {
    div()
        .text_size(px(10.0))
        .font_family("Cascadia Mono")
        .text_color(color)
        .child(bottom_val.to_owned())
}

fn side_value(value: &str, color: gpui::Rgba) -> Div {
    div()
        .px_1()
        .text_size(px(10.0))
        .font_family("Cascadia Mono")
        .text_color(color)
        .child(value.to_owned())
}

fn section_title(title: &str) -> Div {
    div()
        .pt_2()
        .pb_1()
        .text_color(theme::colors().accent)
        .child(title.to_owned())
}

fn pixels(value: gpui::Pixels) -> String {
    format!("{:.1}px", f32::from(value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_edges_correctly_parses_json_structure() {
        let value = json!({
            "margin": {
                "top": { "Pixels": 12.0 },
                "bottom": { "Pixels": 8.0 },
                "left": 4.0,
                "right": null
            },
            "padding": {
                "top": 16.0,
                "bottom": 16.0,
                "left": 24.0,
                "right": 24.0
            }
        });

        let margin = extract_edges(&value, "margin");
        assert_eq!(margin.top, "12");
        assert_eq!(margin.bottom, "8");
        assert_eq!(margin.left, "4");
        assert_eq!(margin.right, "-");

        let padding = extract_edges(&value, "padding");
        assert_eq!(padding.top, "16");
        assert_eq!(padding.right, "24");
        assert_eq!(padding.bottom, "16");
        assert_eq!(padding.left, "24");
    }
}
