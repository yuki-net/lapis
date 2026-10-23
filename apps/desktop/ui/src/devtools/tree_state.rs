//! 描画済みsnapshotから作るTreeの表示状態。GPUIにはUIの折り畳み状態を持たせない。
use gpui::{InspectorElementId, InspectorElementNode};
use std::{
    collections::{HashMap, HashSet},
    rc::Rc,
};

#[derive(Clone)]
pub(super) struct TreeRow {
    pub node: InspectorElementNode,
    pub depth: usize,
    pub has_children: bool,
    pub collapsed: bool,
}

#[derive(Default)]
pub(super) struct TreeState {
    revision: Option<u64>,
    collapsed: HashSet<InspectorElementId>,
    parents: HashMap<InspectorElementId, Option<InspectorElementId>>,
    selected: Option<InspectorElementId>,
    rows: Rc<[TreeRow]>,
    dirty: bool,
    was_picking: bool,
    width_index: usize,
    scroll_target: Option<usize>,
}

impl TreeState {
    pub fn toggle(&mut self, id: InspectorElementId) {
        if !self.collapsed.remove(&id) {
            self.collapsed.insert(id);
        }
        self.dirty = true;
    }

    pub fn rows(
        &mut self,
        nodes: &[InspectorElementNode],
        revision: u64,
        selected: Option<&InspectorElementId>,
        picking: bool,
    ) -> Rc<[TreeRow]> {
        if self.revision != Some(revision) {
            self.parents = nodes
                .iter()
                .map(|node| (node.id.clone(), node.parent.clone()))
                .collect();
            self.collapsed.retain(|id| self.parents.contains_key(id));
            self.revision = Some(revision);
            self.dirty = true;
        }
        // Pick確定時に祖先を展開する。単なるpointer移動では折り畳みを変えない。
        let reveal = !picking && (self.was_picking || self.selected.as_ref() != selected);
        if reveal {
            self.selected = selected.cloned();
            let mut current = selected;
            for _ in 0..self.parents.len() {
                let Some(parent) = current
                    .and_then(|id| self.parents.get(id))
                    .and_then(Option::as_ref)
                else {
                    break;
                };
                self.dirty |= self.collapsed.remove(parent);
                current = Some(parent);
            }
        }
        self.was_picking = picking;
        if self.dirty {
            let parents_with_children: HashSet<_> = nodes
                .iter()
                .filter_map(|node| node.parent.as_ref())
                .collect();
            let mut positions = HashMap::<&InspectorElementId, (usize, bool)>::new();
            let mut rows = Vec::new();
            for node in nodes {
                let (depth, visible) = node
                    .parent
                    .as_ref()
                    .and_then(|parent| {
                        positions.get(parent).map(|(depth, visible)| {
                            (depth + 1, *visible && !self.collapsed.contains(parent))
                        })
                    })
                    .unwrap_or((0, true));
                positions.insert(&node.id, (depth, visible));
                if visible {
                    rows.push(TreeRow {
                        node: node.clone(),
                        depth,
                        has_children: parents_with_children.contains(&node.id),
                        collapsed: self.collapsed.contains(&node.id),
                    });
                }
            }
            // 幅の測定候補もsnapshot変更時だけ選ぶ。描画のたびに全行を走査しない。
            self.width_index = rows
                .iter()
                .enumerate()
                .max_by_key(|(_, row)| {
                    row.depth * 14
                        + row.node.id.path.global_id.to_string().chars().count() * 10
                        + row.node.element_type.len() * 7
                })
                .map_or(0, |(index, _)| index);
            self.rows = rows.into();
            self.dirty = false;
        }
        if reveal {
            self.scroll_target = selected
                .and_then(|selected| self.rows.iter().position(|row| &row.node.id == selected));
        }
        self.rows.clone()
    }

    pub fn width_index(&self) -> usize {
        self.width_index
    }
    pub fn take_scroll_target(&mut self) -> Option<usize> {
        self.scroll_target.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Bounds, GlobalElementId, InspectorElementPath};

    fn node(instance_id: usize, parent: Option<&InspectorElementId>) -> InspectorElementNode {
        InspectorElementNode {
            id: InspectorElementId {
                path: Rc::new(InspectorElementPath {
                    global_id: GlobalElementId::default(),
                    source_location: std::panic::Location::caller(),
                }),
                instance_id,
            },
            parent: parent.cloned(),
            element_type: "Div",
            bounds: Bounds::default(),
        }
    }

    #[test]
    fn collapse_pick_selection_cache_and_disappeared_nodes() {
        let parent = node(0, None);
        let child = node(1, Some(&parent.id));
        let nodes = [parent.clone(), child.clone()];
        let mut state = TreeState::default();
        let rows = state.rows(&nodes, 1, None, false);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].depth, 1);
        assert!(rows[0].has_children);
        assert!(Rc::ptr_eq(&rows, &state.rows(&nodes, 1, None, false)));
        state.toggle(parent.id.clone());
        assert_eq!(state.rows(&nodes, 1, Some(&child.id), true).len(), 1);
        assert_eq!(state.rows(&nodes, 1, Some(&child.id), false).len(), 2);
        state.toggle(parent.id.clone());
        assert_eq!(state.rows(&nodes, 1, Some(&child.id), false).len(), 1);
        assert!(state.rows(&[], 2, None, false).is_empty());
        assert!(state.collapsed.is_empty());
        assert_eq!(state.rows(&nodes, 3, None, false).len(), 2);
    }

    #[test]
    fn independent_ui_states_do_not_share_collapse() {
        let parent = node(0, None);
        let child = node(1, Some(&parent.id));
        let nodes = [parent.clone(), child];
        let mut first = TreeState::default();
        let mut second = TreeState::default();
        first.toggle(parent.id);
        assert_eq!(first.rows(&nodes, 1, None, false).len(), 1);
        assert_eq!(second.rows(&nodes, 1, None, false).len(), 2);
    }
}
