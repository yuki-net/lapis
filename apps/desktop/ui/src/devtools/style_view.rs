use gpui::{Div, StyleRefinement, div, prelude::*, px};
use serde_json::Value;

use crate::theme;

const UNSPECIFIED: &str = "未指定";

struct FieldSpec {
    label: &'static str,
    path: &'static [&'static str],
}

struct SectionSpec {
    title: &'static str,
    fields: &'static [FieldSpec],
}

const LAYOUT_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        label: "display",
        path: &["display"],
    },
    FieldSpec {
        label: "flex direction",
        path: &["flex_direction"],
    },
    FieldSpec {
        label: "align items",
        path: &["align_items"],
    },
    FieldSpec {
        label: "align self",
        path: &["align_self"],
    },
    FieldSpec {
        label: "align content",
        path: &["align_content"],
    },
    FieldSpec {
        label: "justify content",
        path: &["justify_content"],
    },
    FieldSpec {
        label: "gap",
        path: &["gap"],
    },
];

const SPACING_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        label: "margin",
        path: &["margin"],
    },
    FieldSpec {
        label: "padding",
        path: &["padding"],
    },
];

const POSITION_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        label: "position",
        path: &["position"],
    },
    FieldSpec {
        label: "top",
        path: &["inset", "top"],
    },
    FieldSpec {
        label: "right",
        path: &["inset", "right"],
    },
    FieldSpec {
        label: "bottom",
        path: &["inset", "bottom"],
    },
    FieldSpec {
        label: "left",
        path: &["inset", "left"],
    },
    FieldSpec {
        label: "overflow",
        path: &["overflow"],
    },
];

const APPEARANCE_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        label: "background",
        path: &["background"],
    },
    FieldSpec {
        label: "border widths",
        path: &["border_widths"],
    },
    FieldSpec {
        label: "border color",
        path: &["border_color"],
    },
    FieldSpec {
        label: "border style",
        path: &["border_style"],
    },
    FieldSpec {
        label: "corner radius",
        path: &["corner_radii"],
    },
    FieldSpec {
        label: "shadow",
        path: &["box_shadow"],
    },
];

const TEXT_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        label: "color",
        path: &["text", "color"],
    },
    FieldSpec {
        label: "font family",
        path: &["text", "font_family"],
    },
    FieldSpec {
        label: "font size",
        path: &["text", "font_size"],
    },
    FieldSpec {
        label: "font weight",
        path: &["text", "font_weight"],
    },
    FieldSpec {
        label: "font style",
        path: &["text", "font_style"],
    },
    FieldSpec {
        label: "line height",
        path: &["text", "line_height"],
    },
];

const SECTIONS: &[SectionSpec] = &[
    SectionSpec {
        title: "Layout",
        fields: LAYOUT_FIELDS,
    },
    SectionSpec {
        title: "Spacing",
        fields: SPACING_FIELDS,
    },
    SectionSpec {
        title: "Position / Overflow",
        fields: POSITION_FIELDS,
    },
    SectionSpec {
        title: "Appearance",
        fields: APPEARANCE_FIELDS,
    },
    SectionSpec {
        title: "Text",
        fields: TEXT_FIELDS,
    },
];

pub(super) fn render_style(style: &StyleRefinement) -> Div {
    let value = serde_json::to_value(style).unwrap_or(Value::Null);
    let json = serde_json::to_string_pretty(style)
        .unwrap_or_else(|error| format!("StyleRefinementのJSON生成に失敗しました: {error}"));

    div()
        .flex()
        .flex_col()
        .gap_3()
        .children(
            SECTIONS
                .iter()
                .map(|section| render_section(section, &value)),
        )
        .child(render_json(json))
}

pub(super) fn detail_row(label: impl Into<String>, value: impl Into<String>) -> Div {
    div()
        .flex()
        .items_start()
        .gap_3()
        .py_1()
        .border_b_1()
        .border_color(theme::colors().border_default)
        .child(
            div()
                .w(px(132.0))
                .flex_none()
                .text_color(theme::colors().text_secondary)
                .child(label.into()),
        )
        .child(
            div()
                .min_w(px(0.0))
                .flex_1()
                .font_family("Cascadia Mono")
                .text_color(theme::colors().text_primary)
                .child(value.into()),
        )
}

fn render_section(section: &SectionSpec, value: &Value) -> Div {
    div()
        .flex()
        .flex_col()
        .child(section_title(section.title))
        .children(
            section
                .fields
                .iter()
                .map(|field| detail_row(field.label, value_at_path(value, field.path))),
        )
}

fn section_title(title: &str) -> Div {
    div()
        .pt_2()
        .pb_1()
        .text_color(theme::colors().text_accent)
        .child(title.to_owned())
}

fn render_json(json: String) -> Div {
    div()
        .flex()
        .flex_col()
        .child(section_title("StyleRefinement JSON"))
        .child(
            div()
                .flex()
                .flex_col()
                .gap_0p5()
                .p_2()
                .rounded_md()
                .bg(theme::colors().background_primary)
                .font_family("Cascadia Mono")
                .text_color(theme::colors().text_secondary)
                .children(json.lines().map(|line| div().child(line.to_owned()))),
        )
}

fn value_at_path(root: &Value, path: &[&str]) -> String {
    let value = path.iter().try_fold(root, |value, key| value.get(*key));
    match value {
        None | Some(Value::Null) => UNSPECIFIED.to_owned(),
        Some(Value::String(value)) => value.clone(),
        Some(value) => serde_json::to_string(value).unwrap_or_else(|_| UNSPECIFIED.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn style_value_distinguishes_unspecified_from_explicit_values() {
        let style = json!({
            "display": "Flex",
            "align_items": null,
            "inset": { "top": { "Pixels": 8.0 } }
        });

        assert_eq!(value_at_path(&style, &["display"]), "Flex");
        assert_eq!(value_at_path(&style, &["align_items"]), UNSPECIFIED);
        assert_eq!(
            value_at_path(&style, &["inset", "top"]),
            r#"{"Pixels":8.0}"#
        );
        assert_eq!(value_at_path(&style, &["padding"]), UNSPECIFIED);
    }
}
