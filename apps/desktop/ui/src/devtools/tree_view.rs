use std::collections::{HashMap, HashSet};

use gpui::{
    Div, Inspector, InspectorElementId, InspectorElementNode, Stateful, div, prelude::*, px,
};

use crate::theme;

use super::{inspector_controller::InspectorController, style_view::detail_row};

#[derive(Clone)]
struct TreeRow {
    node: InspectorElementNode,
    depth: usize,
    has_children: bool,
}

pub(super) fn render_tree(inspector: &mut Inspector, cx: &mut gpui::Context<Inspector>) -> Div {
    let rows = visible_rows(inspector);
    let node_count = inspector.element_tree().len();
    let revision = inspector.tree_revision();
    let active = inspector.active_element_id().cloned();

    div()
        .size_full()
        .flex()
        .flex_col()
        .child(
            div()
                .h(px(38.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_between()
                .px_3()
                .border_b_1()
                .border_color(theme::border())
                .child("Element tree")
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(theme::muted())
                        .child(format!("{node_count} nodes · rev {revision}")),
                ),
        )
        .child(
            div()
                .id("inspector-tree-scroll")
                .min_h(px(0.0))
                .flex_1()
                .overflow_x_scroll()
                .overflow_y_scroll()
                .py_1()
                .when(rows.is_empty(), |element| {
                    element.child(
                        div()
                            .p_3()
                            .text_size(px(11.0))
                            .text_color(theme::muted())
                            .child("対象ウィンドウを描画するとツリーが表示されます"),
                    )
                })
                .children(rows.into_iter().enumerate().map(|(index, row)| {
                    render_tree_row(index, row, active.as_ref(), inspector, cx)
                })),
        )
}

pub(super) fn render_active_summary(inspector: &Inspector) -> Option<Div> {
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
            .child(section_title("Element"))
            .child(detail_row("type", short_type_name(node.element_type)))
            .child(detail_row(
                "GlobalElementId",
                node.id.path.global_id.to_string(),
            ))
            .child(detail_row("instance ID", node.id.instance_id.to_string()))
            .child(detail_row(
                "生成元",
                format!("{}:{}:{}", source.file(), source.line(), source.column()),
            ))
            .child(section_title("Bounds"))
            .child(detail_row("x", pixels(bounds.origin.x)))
            .child(detail_row("y", pixels(bounds.origin.y)))
            .child(detail_row("width", pixels(bounds.size.width)))
            .child(detail_row("height", pixels(bounds.size.height))),
    )
}

fn visible_rows(inspector: &Inspector) -> Vec<TreeRow> {
    let nodes = inspector.element_tree();
    let parents_with_children = nodes
        .iter()
        .filter_map(|node| node.parent.clone())
        .collect::<HashSet<_>>();
    let mut depths = HashMap::<InspectorElementId, usize>::new();
    let mut visible = HashMap::<InspectorElementId, bool>::new();
    let mut rows = Vec::with_capacity(nodes.len());

    for node in nodes {
        let depth = node
            .parent
            .as_ref()
            .and_then(|parent| depths.get(parent).copied())
            .map_or(0, |depth| depth + 1);
        let is_visible = node.parent.as_ref().is_none_or(|parent| {
            visible.get(parent).copied().unwrap_or(true)
                && !inspector.is_tree_node_collapsed(parent)
        });
        depths.insert(node.id.clone(), depth);
        visible.insert(node.id.clone(), is_visible);

        if is_visible {
            rows.push(TreeRow {
                node: node.clone(),
                depth,
                has_children: parents_with_children.contains(&node.id),
            });
        }
    }
    rows
}

fn render_tree_row(
    index: usize,
    row: TreeRow,
    active: Option<&InspectorElementId>,
    inspector: &mut Inspector,
    cx: &mut gpui::Context<Inspector>,
) -> Stateful<Div> {
    let id = row.node.id.clone();
    let toggle_id = id.clone();
    let selected = active == Some(&id);
    let collapsed = inspector.is_tree_node_collapsed(&id);
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
        .when(selected, |element| element.bg(theme::accent_soft()))
        .hover(|style| style.bg(theme::surface_hover()))
        .child(
            div()
                .id(("inspector-tree-toggle", index))
                .w(px(14.0))
                .flex_none()
                .text_color(theme::muted())
                .child(if row.has_children {
                    if collapsed { "▸" } else { "▾" }
                } else {
                    ""
                })
                .when(row.has_children, |element| {
                    element.on_click(cx.listener(move |inspector, _, _, cx| {
                        inspector.toggle_tree_node(toggle_id.clone());
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
                    theme::text()
                } else {
                    theme::accent()
                })
                .child(short_type_name(row.node.element_type)),
        )
        .when(!global_id.is_empty(), |element| {
            element.child(
                div()
                    .min_w(px(0.0))
                    .text_size(px(9.0))
                    .text_color(theme::muted())
                    .child(format!("#{global_id}")),
            )
        })
        .on_click(cx.listener(move |_, _, _, cx| {
            let id = id.clone();
            cx.defer(move |cx| InspectorController::select_element(id, cx));
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
