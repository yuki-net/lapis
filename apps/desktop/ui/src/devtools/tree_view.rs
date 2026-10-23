use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui::{
    Div, Inspector, InspectorElementId, InspectorElementNode, Stateful, div, prelude::*, px,
};

use crate::{components::uniform_list_viewport, theme};

use super::{inspector_controller::InspectorController, style_view::detail_row};

#[derive(Clone)]
pub(super) struct TreeRow {
    node: InspectorElementNode,
    depth: usize,
    has_children: bool,
    collapsed: bool,
}

#[derive(Default)]
pub(super) struct TreeState {
    collapsed: HashSet<InspectorElementId>,
    revision: Option<u64>,
    active: Option<InspectorElementId>,
    rows: Rc<Vec<TreeRow>>,
}

impl TreeState {
    pub(super) fn toggle(&mut self, id: InspectorElementId) {
        if !self.collapsed.remove(&id) {
            self.collapsed.insert(id);
        }
        self.revision = None;
    }

    pub(super) fn rows(&mut self, inspector: &Inspector) -> Rc<Vec<TreeRow>> {
        let active = inspector.active_element_id();
        if self.revision == Some(inspector.tree_revision()) && self.active.as_ref() == active {
            return self.rows.clone();
        }
        let nodes = inspector.element_tree();
        let parents: HashMap<_, _> = nodes
            .iter()
            .map(|node| (&node.id, node.parent.as_ref()))
            .collect();
        self.collapsed.retain(|id| parents.contains_key(id));
        if self.active.as_ref() != active {
            let mut current = active;
            while let Some(parent) = current.and_then(|id| parents.get(id).copied().flatten()) {
                self.collapsed.remove(parent);
                current = Some(parent);
            }
        }
        self.active = active.cloned();
        self.revision = Some(inspector.tree_revision());
        self.rows = Rc::new(visible_rows(nodes, &self.collapsed));
        self.rows.clone()
    }
}

pub(super) fn render_tree(inspector: &mut Inspector, cx: &mut gpui::Context<Inspector>) -> Div {
    let rows = InspectorController::ui(cx).tree.rows(inspector);
    let node_count = inspector.element_tree().len();
    let revision = inspector.tree_revision();
    let active = inspector.active_element_id().cloned();
    let scroll_state = InspectorController::tree_scroll(cx);

    let empty = rows.is_empty();
    let tree_content = gpui::uniform_list(
        "inspector-tree-scroll",
        rows.len(),
        cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
            range
                .map(|index| render_tree_row(index, rows[index].clone(), active.as_ref(), cx))
                .collect()
        }),
    )
    .with_horizontal_sizing_behavior(gpui::ListHorizontalSizingBehavior::Unconstrained);

    let tree_content = if empty {
        div()
            .p_3()
            .text_size(px(11.0))
            .text_color(theme::colors().text_secondary)
            .child("対象ウィンドウを描画するとツリーが表示されます")
    } else {
        uniform_list_viewport(&scroll_state, tree_content)
    };

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
                .border_color(theme::colors().border_default)
                .child("Element tree")
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(theme::colors().text_secondary)
                        .child(format!("{node_count} nodes · rev {revision}")),
                ),
        )
        .child(tree_content.min_h(px(0.0)).flex_1())
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

fn visible_rows(
    nodes: &[InspectorElementNode],
    collapsed: &HashSet<InspectorElementId>,
) -> Vec<TreeRow> {
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
            visible.get(parent).copied().unwrap_or(true) && !collapsed.contains(parent)
        });
        depths.insert(node.id.clone(), depth);
        visible.insert(node.id.clone(), is_visible);

        if is_visible {
            rows.push(TreeRow {
                node: node.clone(),
                depth,
                has_children: parents_with_children.contains(&node.id),
                collapsed: collapsed.contains(&node.id),
            });
        }
    }
    rows
}

fn render_tree_row(
    index: usize,
    row: TreeRow,
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
        .debug_selector(move || format!("inspector-tree-row-{index}"))
        .h(px(26.0))
        .min_w(px(280.0))
        .flex()
        .items_center()
        .gap_1()
        .pl(px(8.0 + row.depth as f32 * 14.0))
        .pr_2()
        .cursor_pointer()
        .when(selected, |element| {
            element.bg(theme::colors().button_background_selected)
        })
        .hover(|style| style.bg(theme::colors().button_background_hover))
        .child(
            div()
                .id(("inspector-tree-toggle", index))
                .debug_selector(move || format!("inspector-tree-toggle-{index}"))
                .w(px(14.0))
                .flex_none()
                .text_color(theme::colors().text_secondary)
                .child(if row.has_children {
                    if collapsed { "▸" } else { "▾" }
                } else {
                    ""
                })
                .when(row.has_children, |element| {
                    element.on_click(cx.listener(move |_, _, _, cx| {
                        InspectorController::ui(cx).tree.toggle(toggle_id.clone());
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
                    theme::colors().text_primary
                } else {
                    theme::colors().text_accent
                })
                .child(short_type_name(row.node.element_type)),
        )
        .when(!global_id.is_empty(), |element| {
            element.child(
                div()
                    .min_w(px(0.0))
                    .text_size(px(9.0))
                    .text_color(theme::colors().text_secondary)
                    .child(format!("#{global_id}")),
            )
        })
        .on_hover(cx.listener(move |_, hovered: &bool, _, cx| {
            let session = cx.entity().entity_id();
            let id = hovered.then(|| hover_id.clone());
            cx.defer(move |cx| {
                InspectorController::hover_element(session, id, cx);
            });
        }))
        .on_click(cx.listener(move |_, _, _, cx| {
            let session = cx.entity().entity_id();
            let id = id.clone();
            cx.defer(move |cx| InspectorController::select_element(session, id, cx));
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
        .text_color(theme::colors().text_accent)
        .child(title.to_owned())
}

fn pixels(value: gpui::Pixels) -> String {
    format!("{:.1}px", f32::from(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_node(instance_id: usize, parent: Option<InspectorElementId>) -> InspectorElementNode {
        InspectorElementNode {
            id: InspectorElementId {
                path: Rc::new(gpui::InspectorElementPath {
                    global_id: Default::default(),
                    source_location: std::panic::Location::caller(),
                }),
                instance_id,
            },
            parent,
            element_type: "test::Div",
            bounds: Default::default(),
        }
    }

    #[test]
    fn collapsed_nodes_hide_only_their_descendants() {
        let root = test_node(0, None);
        let parent = test_node(1, Some(root.id.clone()));
        let leaf = test_node(2, Some(parent.id.clone()));
        let sibling = test_node(3, Some(root.id.clone()));
        let nodes = [root.clone(), parent.clone(), leaf.clone(), sibling.clone()];
        let rows = visible_rows(&nodes, &HashSet::from([parent.id.clone()]));
        assert_eq!(
            rows.iter()
                .map(|row| row.node.id.clone())
                .collect::<Vec<_>>(),
            vec![root.id, parent.id.clone(), sibling.id]
        );
        assert_eq!(
            rows.iter().map(|row| row.depth).collect::<Vec<_>>(),
            vec![0, 1, 1]
        );
        assert!(rows[1].has_children);
        assert!(rows[1].collapsed);
        let rows = visible_rows(&nodes, &HashSet::new());
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[2].node.id, leaf.id);
        assert_eq!(rows[2].depth, 2);
    }

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
