use gpui::{Div, Inspector, InspectorElementId, Stateful, div, prelude::*, px};

use crate::{
    components::{ScrollAxis, panel_header, scroll_surface},
    theme,
};

use super::tree_state::TreeRow;
use super::{inspector_controller::InspectorController, style_view::detail_row};

pub(super) fn render_tree(inspector: &mut Inspector, cx: &mut gpui::Context<Inspector>) -> Div {
    let entity = cx.entity_id();
    let rows = InspectorController::tree_rows(inspector, entity, cx);
    let node_count = inspector.element_tree().len();
    let revision = inspector.tree_revision();
    let active = inspector.active_element_id().cloned();
    let scroll_state = InspectorController::tree_scroll(entity, cx);

    let tree_content = if rows.is_empty() {
        div()
            .p_3()
            .text_size(px(11.))
            .text_color(theme::colors(cx).text_secondary)
            .child("対象ウィンドウを描画するとツリーが表示されます")
            .into_any_element()
    } else {
        let engine = cx.entity();
        let list = InspectorController::tree_list(entity, cx);
        let width_index = InspectorController::tree_width_index(entity, cx);
        let wheel_state = scroll_state.clone();
        gpui::uniform_list("inspector-tree-list", rows.len(), move |range, _, cx| {
            engine.update(cx, |_, cx| {
                range
                    .map(|index| render_tree_row(index, &rows[index], active.as_ref(), cx))
                    .collect::<Vec<_>>()
            })
        })
        .size_full()
        .py_1()
        .with_width_from_item(Some(width_index))
        .with_horizontal_sizing_behavior(gpui::ListHorizontalSizingBehavior::Unconstrained)
        .track_scroll(list)
        .on_scroll_wheel(move |_, _, _| wheel_state.reveal())
        .into_any_element()
    };

    div()
        .size_full()
        .flex()
        .flex_col()
        .child(
            panel_header()
                .justify_between()
                .px_3()
                .child("Element tree")
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(theme::colors(cx).text_secondary)
                        .child(format!("{node_count} nodes · rev {revision}")),
                ),
        )
        .child(
            scroll_surface(ScrollAxis::Both, &scroll_state, tree_content, cx)
                .min_h(px(0.0))
                .flex_1(),
        )
}
pub(super) fn render_active_summary(inspector: &Inspector, cx: &gpui::App) -> Option<Div> {
    let active = inspector.active_element_id()?;
    let node = inspector
        .element_tree()
        .iter()
        .find(|node| &node.id == active)?;
    let source = node.id.path.source_location;
    let bounds = node.bounds;

    Some(
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(section_title("Element", cx))
            .child(detail_row("type", short_type_name(node.element_type), cx))
            .child(detail_row(
                "GlobalElementId",
                node.id.path.global_id.to_string(),
                cx,
            ))
            .child(detail_row(
                "instance ID",
                node.id.instance_id.to_string(),
                cx,
            ))
            .child(detail_row(
                "生成元",
                format!("{}:{}:{}", source.file(), source.line(), source.column()),
                cx,
            ))
            .child(section_title("Bounds", cx))
            .child(detail_row("x", pixels(bounds.origin.x), cx))
            .child(detail_row("y", pixels(bounds.origin.y), cx))
            .child(detail_row("width", pixels(bounds.size.width), cx))
            .child(detail_row("height", pixels(bounds.size.height), cx)),
    )
}

fn render_tree_row(
    index: usize,
    row: &TreeRow,
    active: Option<&InspectorElementId>,
    cx: &mut gpui::Context<Inspector>,
) -> Stateful<Div> {
    let id = row.node.id.clone();
    let hover_id = id.clone();
    let toggle_id = id.clone();
    let selected = active == Some(&id);
    let collapsed = row.collapsed;
    let global_id = row.node.id.path.global_id.to_string();

    div()
        .id(("inspector-tree-row", index))
        .h(px(26.0))
        .min_w(px(280.0))
        .flex()
        .items_center()
        .gap_1()
        .pl(px(8.0 + row.depth as f32 * 14.0))
        .pr_2()
        .cursor_pointer()
        .when(selected, |element| {
            element.bg(theme::colors(cx).button_background_selected)
        })
        .hover(|style| style.bg(theme::colors(cx).button_background_hover))
        .child(
            div()
                .id(("inspector-tree-toggle", index))
                .w(px(14.0))
                .flex_none()
                .text_color(theme::colors(cx).text_secondary)
                .child(if row.has_children {
                    if collapsed { "▸" } else { "▾" }
                } else {
                    ""
                })
                .when(row.has_children, |element| {
                    element.on_click(cx.listener(move |_, _, _, cx| {
                        InspectorController::toggle_tree_node(
                            toggle_id.clone(),
                            cx.entity_id(),
                            cx,
                        );
                        cx.stop_propagation();
                        cx.notify();
                    }))
                }),
        )
        .child(
            div()
                .flex_none()
                .text_size(px(11.0))
                .text_color(if selected {
                    theme::colors(cx).text_primary
                } else {
                    theme::colors(cx).text_accent
                })
                .child(short_type_name(row.node.element_type)),
        )
        .when(!global_id.is_empty(), |element| {
            element.child(
                div()
                    .min_w(px(0.0))
                    .text_size(px(9.0))
                    .text_color(theme::colors(cx).text_secondary)
                    .child(format!("#{global_id}")),
            )
        })
        .on_hover(cx.listener(move |_, hovered: &bool, _, cx| {
            let id = hovered.then(|| hover_id.clone());
            InspectorController::defer(cx, move |entity, cx| {
                InspectorController::hover_element(id, entity, cx);
            });
        }))
        .on_click(cx.listener(move |_, _, _, cx| {
            let id = id.clone();
            InspectorController::defer(cx, move |entity, cx| {
                InspectorController::select_element(id, entity, cx)
            });
        }))
}

fn short_type_name(element_type: &'static str) -> String {
    if element_type.contains("::div::Div") {
        return "div".to_owned();
    }
    if element_type.contains("::svg::Svg") {
        return "svg".to_owned();
    }
    if element_type.contains("::img::Img") {
        return "img".to_owned();
    }
    if let Some(component) = element_type
        .strip_prefix("gpui::element::Component<")
        .and_then(|name| name.strip_suffix('>'))
    {
        return component
            .rsplit("::")
            .next()
            .unwrap_or(component)
            .to_owned();
    }
    element_type
        .rsplit("::")
        .next()
        .unwrap_or(element_type)
        .to_owned()
}

fn section_title(title: &str, cx: &gpui::App) -> Div {
    div()
        .pt_2()
        .pb_1()
        .text_color(theme::colors(cx).text_accent)
        .child(title.to_owned())
}

fn pixels(value: gpui::Pixels) -> String {
    format!("{:.1}px", f32::from(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_type_name_prefers_dom_like_names() {
        assert_eq!(short_type_name("gpui::elements::div::Div"), "div");
        assert_eq!(short_type_name("gpui::elements::svg::Svg"), "svg");
        assert_eq!(
            short_type_name("gpui::element::Component<lapis::widgets::SearchBox>"),
            "SearchBox"
        );
    }
}
