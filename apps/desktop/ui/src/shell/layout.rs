use std::collections::{BTreeMap, BTreeSet, HashSet};

use lapis_desktop_client::{LayoutGroupViewState, LayoutNodeViewState, LayoutViewState};

use crate::extension_ui::PanelPosition;

use super::{
    panel::{PanelHost, panel_size},
    tab_state::PanelTab,
};

const REQUIRED_ANCHORS: [PanelPosition; 4] = [
    PanelPosition::Main,
    PanelPosition::Left,
    PanelPosition::Bottom,
    PanelPosition::Right,
];

/// A leaf in the layout tree is a tab group. The tree deliberately has no
/// fixed number of panels; the four positions are only compatibility anchors
/// used by the existing header commands.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct TabGroupId(u64);

impl TabGroupId {
    pub(crate) fn new(value: u64) -> Self {
        Self(value)
    }

    pub(crate) fn as_str(&self) -> String {
        format!("group-{}", self.0)
    }

    fn parse(value: &str) -> Option<Self> {
        value.strip_prefix("group-")?.parse().ok().map(Self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum LayoutNode {
    Tabs(TabGroupId),
    Split {
        direction: SplitDirection,
        ratio: f32,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LayoutError {
    GroupNotFound,
    GroupIsNotLeaf,
    InvalidRatio,
    MainGroupRequired,
    InvalidTree,
    TabNotFound,
}

/// Owns placement and visibility state only. A `PanelHost` may contain a
/// Document or Tool tab, but closing or moving it never touches the Host
/// resource behind the tab.
#[derive(Clone, Debug)]
pub(crate) struct DockLayout {
    root: LayoutNode,
    groups: BTreeMap<TabGroupId, PanelHost>,
    primary_groups: BTreeMap<PanelPosition, TabGroupId>,
    next_group_id: u64,
}

impl Default for DockLayout {
    fn default() -> Self {
        let main = TabGroupId::new(1);
        let left = TabGroupId::new(2);
        let bottom = TabGroupId::new(3);
        let right = TabGroupId::new(4);
        let mut groups = BTreeMap::new();
        groups.insert(
            main.clone(),
            PanelHost::new(PanelPosition::Main, Vec::new(), true, 0.0),
        );
        groups.insert(
            left.clone(),
            PanelHost::new(
                PanelPosition::Left,
                vec![PanelTab::tool(crate::features::id::VIEW_FILES)],
                true,
                f32::from(crate::tokens::size::TOOL_ISLAND_WIDTH),
            ),
        );
        groups.insert(
            bottom.clone(),
            PanelHost::new(
                PanelPosition::Bottom,
                Vec::new(),
                false,
                f32::from(crate::tokens::size::BOTTOM_PANEL_HEIGHT),
            ),
        );
        groups.insert(
            right.clone(),
            PanelHost::new(
                PanelPosition::Right,
                Vec::new(),
                false,
                f32::from(crate::tokens::size::SIDE_PANEL_WIDTH),
            ),
        );

        let side = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.78,
            first: Box::new(LayoutNode::Split {
                direction: SplitDirection::Vertical,
                ratio: 0.76,
                first: Box::new(LayoutNode::Tabs(main.clone())),
                second: Box::new(LayoutNode::Tabs(bottom.clone())),
            }),
            second: Box::new(LayoutNode::Tabs(right.clone())),
        };
        let root = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.22,
            first: Box::new(LayoutNode::Tabs(left.clone())),
            second: Box::new(side),
        };
        let primary_groups = BTreeMap::from([
            (PanelPosition::Main, main),
            (PanelPosition::Left, left),
            (PanelPosition::Bottom, bottom),
            (PanelPosition::Right, right),
        ]);
        Self {
            root,
            groups,
            primary_groups,
            next_group_id: 5,
        }
    }
}

impl DockLayout {
    pub(crate) fn group(&self, id: &TabGroupId) -> Option<&PanelHost> {
        self.groups.get(id)
    }

    pub(crate) fn group_mut(&mut self, id: &TabGroupId) -> Option<&mut PanelHost> {
        self.groups.get_mut(id)
    }

    pub(crate) fn group_for_tab(&self, tab: &PanelTab) -> Option<TabGroupId> {
        self.groups
            .iter()
            .find_map(|(id, group)| group.contains(tab).then(|| id.clone()))
    }

    pub(crate) fn placements(&self) -> Vec<(PanelTab, TabGroupId)> {
        self.groups
            .iter()
            .flat_map(|(id, group)| group.tabs.iter().cloned().map(move |tab| (tab, id.clone())))
            .collect()
    }

    pub(crate) fn primary_group_id(&self, position: PanelPosition) -> &TabGroupId {
        self.primary_groups
            .get(&position)
            .expect("DockLayout must retain a primary group for every header position")
    }

    pub(crate) fn leaf_group_ids(&self) -> Vec<TabGroupId> {
        let mut ids = Vec::new();
        collect_leaf_ids(&self.root, &mut ids);
        ids
    }

    pub(crate) fn panels(&self) -> Vec<&PanelHost> {
        self.leaf_group_ids()
            .iter()
            .map(|id| {
                self.group(id)
                    .expect("layout leaf must have a corresponding tab group")
            })
            .collect()
    }

    pub(crate) fn panels_mut(&mut self) -> impl Iterator<Item = &mut PanelHost> {
        self.groups.values_mut()
    }

    pub(crate) fn panel(&self, position: PanelPosition) -> &PanelHost {
        let id = self.primary_group_id(position);
        self.group(id).expect("primary layout group must exist")
    }

    pub(crate) fn panel_mut(&mut self, position: PanelPosition) -> &mut PanelHost {
        let id = self.primary_group_id(position).clone();
        self.groups
            .get_mut(&id)
            .expect("primary layout group must exist")
    }

    /// Splits a leaf without moving or cloning its existing tabs.
    #[allow(dead_code)]
    pub(crate) fn split_group(
        &mut self,
        group: &TabGroupId,
        direction: SplitDirection,
        ratio: f32,
    ) -> Result<TabGroupId, LayoutError> {
        if !ratio.is_finite() || !(0.1..=0.9).contains(&ratio) {
            return Err(LayoutError::InvalidRatio);
        }
        if !self.groups.contains_key(group) {
            return Err(LayoutError::GroupNotFound);
        }
        if !contains_leaf(&self.root, group) {
            return Err(LayoutError::GroupIsNotLeaf);
        }
        let new_id = self.allocate_group_id();
        let source = self.group(group).expect("group checked above");
        self.groups.insert(
            new_id.clone(),
            PanelHost::new(source.position, Vec::new(), true, source.size),
        );
        if !replace_leaf(
            &mut self.root,
            group,
            LayoutNode::Split {
                direction,
                ratio,
                first: Box::new(LayoutNode::Tabs(group.clone())),
                second: Box::new(LayoutNode::Tabs(new_id.clone())),
            },
        ) {
            self.groups.remove(&new_id);
            return Err(LayoutError::InvalidTree);
        }
        Ok(new_id)
    }

    // 保存値の最大IDや削除済みGroupに依存せず、既存Groupを上書きしない。
    fn allocate_group_id(&mut self) -> TabGroupId {
        loop {
            let id = TabGroupId::new(self.next_group_id);
            self.next_group_id = self.next_group_id.wrapping_add(1);
            if !self.groups.contains_key(&id) {
                return id;
            }
        }
    }

    /// Moves display placement only. The source group remains present when it
    /// becomes empty so the main work area and hidden restoration anchors do
    /// not disappear with their last tab.
    pub(crate) fn move_tab(
        &mut self,
        source: &TabGroupId,
        target: &TabGroupId,
        tab: &PanelTab,
    ) -> Result<(), LayoutError> {
        if !self.groups.contains_key(source) || !self.groups.contains_key(target) {
            return Err(LayoutError::GroupNotFound);
        }
        if !self.group(source).is_some_and(|group| group.contains(tab)) {
            return Err(LayoutError::TabNotFound);
        }
        if source == target {
            self.groups
                .get_mut(target)
                .expect("target checked above")
                .activate_without_open(tab.clone());
            return Ok(());
        }
        let removed = self
            .group_mut(source)
            .expect("source checked above")
            .remove(tab);
        if removed {
            self.group_mut(target)
                .expect("target checked above")
                .activate_without_open(tab.clone());
        }
        Ok(())
    }

    pub(crate) fn capture_view_state(&self) -> LayoutViewState {
        debug_assert!(self.validate().is_ok());
        LayoutViewState {
            version: 1,
            anchors: self
                .primary_groups
                .iter()
                .map(|(position, id)| (panel_position_name(*position).to_owned(), id.as_str()))
                .collect(),
            root: capture_node(&self.root),
            groups: self
                .leaf_group_ids()
                .into_iter()
                .filter_map(|id| self.groups.get(&id).map(|group| (id, group)))
                .map(|(id, group)| LayoutGroupViewState {
                    id: id.as_str(),
                    position: panel_position_name(group.position).to_owned(),
                    tabs: group.tabs.iter().map(serialize_tab).collect(),
                    active_tab: group.active.as_ref().map(serialize_tab),
                    open: group.open,
                    size: group.size,
                })
                .collect(),
        }
    }

    /// Restores only layout data. `parse_tab` is supplied by the UI layer so
    /// unknown tools and documents can be rejected without reviving resources.
    pub(crate) fn restore_view_state<F>(
        &mut self,
        state: &LayoutViewState,
        mut parse_tab: F,
    ) -> bool
    where
        F: FnMut(&str) -> Option<PanelTab>,
    {
        if state.version != 1 || state.groups.is_empty() {
            return false;
        }
        let mut groups = BTreeMap::new();
        let mut seen_tabs = HashSet::new();
        for saved in &state.groups {
            let Some(id) = TabGroupId::parse(&saved.id) else {
                return false;
            };
            if groups.contains_key(&id) {
                return false;
            }
            let Some(position) = panel_position(saved.position.as_str()) else {
                return false;
            };
            let mut tabs = Vec::new();
            for raw in &saved.tabs {
                let Some(tab) = parse_tab(raw) else {
                    continue;
                };
                if !seen_tabs.insert(tab.instance_id()) {
                    return false;
                }
                tabs.push(tab);
            }
            let active = saved
                .active_tab
                .as_deref()
                .and_then(&mut parse_tab)
                .filter(|tab| tabs.contains(tab))
                .or_else(|| tabs.first().cloned());
            if !saved.size.is_finite() {
                return false;
            }
            let mut group = PanelHost::new(
                position,
                tabs,
                position == PanelPosition::Main,
                panel_size(position, saved.size),
            );
            group.active = active;
            group.set_open_immediate(position == PanelPosition::Main || saved.open);
            groups.insert(id, group);
        }
        let Some(root) = restore_node(&state.root, &groups) else {
            return false;
        };
        let leaf_ids = {
            let mut ids = Vec::new();
            collect_leaf_ids(&root, &mut ids);
            ids
        };
        if leaf_ids.len() != groups.len() || !leaf_ids.iter().all(|id| groups.contains_key(id)) {
            return false;
        }
        let mut primary_groups = BTreeMap::new();
        for id in &leaf_ids {
            let position = groups.get(id).expect("leaf group checked above").position;
            primary_groups.entry(position).or_insert_with(|| id.clone());
        }
        if !state.anchors.is_empty() {
            primary_groups.clear();
            for (position, id) in &state.anchors {
                let (Some(position), Some(id)) = (panel_position(position), TabGroupId::parse(id))
                else {
                    return false;
                };
                if primary_groups.insert(position, id).is_some() {
                    return false;
                }
            }
        } else {
            // v1の旧保存値では、残っている既定Anchor IDを優先して移行する。
            for (position, id) in &DockLayout::default().primary_groups {
                if groups
                    .get(id)
                    .is_some_and(|group| group.position == *position)
                {
                    primary_groups.insert(*position, id.clone());
                }
            }
        }
        let next_group_id = groups
            .keys()
            .map(|id| id.0)
            .max()
            .unwrap_or(0)
            .wrapping_add(1);
        let restored = Self {
            root,
            groups,
            primary_groups,
            next_group_id,
        };
        // 描画側と同じ不変条件を境界で検査し、不正な保存値は現状態へ適用しない。
        if restored.validate().is_err() {
            return false;
        }
        *self = restored;
        true
    }

    pub(crate) fn validate(&self) -> Result<(), LayoutError> {
        if !valid_ratios(&self.root) {
            return Err(LayoutError::InvalidRatio);
        }
        let mut ids = Vec::new();
        collect_leaf_ids(&self.root, &mut ids);
        if ids.is_empty() || ids.iter().collect::<BTreeSet<_>>().len() != ids.len() {
            return Err(LayoutError::InvalidTree);
        }
        if ids.len() != self.groups.len() || !ids.iter().all(|id| self.groups.contains_key(id)) {
            return Err(LayoutError::InvalidTree);
        }
        if !REQUIRED_ANCHORS.iter().all(|position| {
            self.primary_groups
                .get(position)
                .and_then(|id| self.groups.get(id))
                .is_some_and(|group| group.position == *position)
        }) {
            return Err(LayoutError::MainGroupRequired);
        }
        let mut seen_tabs = HashSet::new();
        for group in self.groups.values() {
            if !group.size.is_finite() || group.size != panel_size(group.position, group.size) {
                return Err(LayoutError::InvalidTree);
            }
            if let Some(active) = &group.active
                && !group.tabs.contains(active)
            {
                return Err(LayoutError::InvalidTree);
            }
            for tab in &group.tabs {
                if tab.instance_id().as_str().is_empty() || !seen_tabs.insert(tab.instance_id()) {
                    return Err(LayoutError::InvalidTree);
                }
            }
        }
        Ok(())
    }
}

fn collect_leaf_ids(node: &LayoutNode, ids: &mut Vec<TabGroupId>) {
    match node {
        LayoutNode::Tabs(id) => ids.push(id.clone()),
        LayoutNode::Split { first, second, .. } => {
            collect_leaf_ids(first, ids);
            collect_leaf_ids(second, ids);
        }
    }
}

fn valid_ratios(node: &LayoutNode) -> bool {
    match node {
        LayoutNode::Tabs(_) => true,
        LayoutNode::Split {
            ratio,
            first,
            second,
            ..
        } => {
            ratio.is_finite()
                && (0.1..=0.9).contains(ratio)
                && valid_ratios(first)
                && valid_ratios(second)
        }
    }
}

#[allow(dead_code)]
fn contains_leaf(node: &LayoutNode, target: &TabGroupId) -> bool {
    match node {
        LayoutNode::Tabs(id) => id == target,
        LayoutNode::Split { first, second, .. } => {
            contains_leaf(first, target) || contains_leaf(second, target)
        }
    }
}

#[allow(dead_code)]
fn replace_leaf(node: &mut LayoutNode, target: &TabGroupId, replacement: LayoutNode) -> bool {
    match node {
        LayoutNode::Tabs(id) if id == target => {
            *node = replacement;
            true
        }
        LayoutNode::Split { first, second, .. } => {
            replace_leaf(first, target, replacement.clone())
                || replace_leaf(second, target, replacement)
        }
        LayoutNode::Tabs(_) => false,
    }
}

fn capture_node(node: &LayoutNode) -> LayoutNodeViewState {
    match node {
        LayoutNode::Tabs(id) => LayoutNodeViewState::Tabs { group: id.as_str() },
        LayoutNode::Split {
            direction,
            ratio,
            first,
            second,
        } => LayoutNodeViewState::Split {
            direction: match direction {
                SplitDirection::Horizontal => "horizontal",
                SplitDirection::Vertical => "vertical",
            }
            .to_owned(),
            ratio: *ratio,
            first: Box::new(capture_node(first)),
            second: Box::new(capture_node(second)),
        },
    }
}

fn restore_node(
    node: &LayoutNodeViewState,
    groups: &BTreeMap<TabGroupId, PanelHost>,
) -> Option<LayoutNode> {
    match node {
        LayoutNodeViewState::Tabs { group } => {
            let id = TabGroupId::parse(group)?;
            groups.contains_key(&id).then_some(LayoutNode::Tabs(id))
        }
        LayoutNodeViewState::Split {
            direction,
            ratio,
            first,
            second,
        } => {
            if !ratio.is_finite() || !(0.1..=0.9).contains(ratio) {
                return None;
            }
            let direction = match direction.as_str() {
                "horizontal" => SplitDirection::Horizontal,
                "vertical" => SplitDirection::Vertical,
                _ => return None,
            };
            Some(LayoutNode::Split {
                direction,
                ratio: *ratio,
                first: Box::new(restore_node(first, groups)?),
                second: Box::new(restore_node(second, groups)?),
            })
        }
    }
}

fn serialize_tab(tab: &PanelTab) -> String {
    let instance = tab.instance_id();
    match tab {
        PanelTab::Document(id) => format!("document:{}", id.as_str()),
        PanelTab::Tool { view, .. } => format!("tool:{}:{}", view.as_str(), instance.as_str()),
    }
}

fn panel_position(value: &str) -> Option<PanelPosition> {
    match value {
        "main-panel" | "center-panel" => Some(PanelPosition::Main),
        "left-panel" => Some(PanelPosition::Left),
        "bottom-panel" => Some(PanelPosition::Bottom),
        "right-panel" => Some(PanelPosition::Right),
        _ => None,
    }
}

fn panel_position_name(position: PanelPosition) -> &'static str {
    match position {
        PanelPosition::Main => "main-panel",
        PanelPosition::Left => "left-panel",
        PanelPosition::Bottom => "bottom-panel",
        PanelPosition::Right => "right-panel",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout_has_legacy_anchors_but_uses_a_tree() {
        let layout = DockLayout::default();
        assert_eq!(layout.leaf_group_ids().len(), 4);
        assert!(matches!(layout.root, LayoutNode::Split { .. }));
        assert!(layout.validate().is_ok());
    }

    #[test]
    fn split_keeps_existing_tabs_and_creates_an_independent_group() {
        let mut layout = DockLayout::default();
        let source = layout.primary_group_id(PanelPosition::Main).clone();
        let tab = PanelTab::tool("terminal");
        layout.group_mut(&source).unwrap().activate(tab.clone());
        let target = layout
            .split_group(&source, SplitDirection::Horizontal, 0.5)
            .unwrap();

        assert!(layout.group(&source).unwrap().contains(&tab));
        assert!(layout.group(&target).unwrap().tabs.is_empty());
        assert_eq!(layout.leaf_group_ids().len(), 5);
        assert!(layout.validate().is_ok());
    }

    #[test]
    fn moving_tabs_does_not_close_or_duplicate_the_source_resource_reference() {
        let mut layout = DockLayout::default();
        let source = layout.primary_group_id(PanelPosition::Left).clone();
        let target = layout.primary_group_id(PanelPosition::Main).clone();
        let tab = PanelTab::tool("terminal");
        layout.group_mut(&source).unwrap().activate(tab.clone());
        layout.move_tab(&source, &target, &tab).unwrap();

        assert!(!layout.group(&source).unwrap().contains(&tab));
        assert!(layout.group(&target).unwrap().contains(&tab));
        assert!(layout.group(&source).unwrap().open);
        assert!(layout.validate().is_ok());
    }

    #[test]
    fn same_view_definition_can_have_multiple_stable_instances() {
        let mut layout = DockLayout::default();
        let main = layout.primary_group_id(PanelPosition::Main).clone();
        let first = PanelTab::tool_instance("terminal", "terminal-instance-1");
        let second = PanelTab::tool_instance("terminal", "terminal-instance-2");
        layout.group_mut(&main).unwrap().activate(first.clone());
        layout.group_mut(&main).unwrap().activate(second.clone());

        assert_ne!(first, second);
        assert_eq!(first.view_id(), second.view_id());
        assert_eq!(layout.group(&main).unwrap().tabs.len(), 2);
        assert!(layout.validate().is_ok());
    }

    #[test]
    fn invalid_saved_tree_is_rejected_without_replacing_current_layout() {
        let mut layout = DockLayout::default();
        let before = layout.capture_view_state();
        let invalid = LayoutViewState {
            version: 1,
            anchors: Default::default(),
            root: LayoutNodeViewState::Tabs {
                group: "group-999".into(),
            },
            groups: Vec::new(),
        };
        assert!(!layout.restore_view_state(&invalid, |_| None));
        assert_eq!(layout.capture_view_state(), before);
    }

    #[test]
    fn missing_compatibility_anchors_are_rejected_atomically() {
        for position in REQUIRED_ANCHORS {
            let mut layout = DockLayout::default();
            let before = layout.capture_view_state();
            let mut saved = before.clone();
            for group in &mut saved.groups {
                if group.position == panel_position_name(position) {
                    group.position = panel_position_name(if position == PanelPosition::Main {
                        PanelPosition::Left
                    } else {
                        PanelPosition::Main
                    })
                    .into();
                }
            }
            assert!(!layout.restore_view_state(&saved, |_| None));
            assert_eq!(layout.capture_view_state(), before);
        }
        let mut layout = DockLayout::default();
        let mut saved = layout.capture_view_state();
        saved.groups.retain(|group| group.position == "main-panel");
        saved.root = LayoutNodeViewState::Tabs {
            group: saved.groups[0].id.clone(),
        };
        assert!(!layout.restore_view_state(&saved, |_| None));
    }

    #[test]
    fn restored_sizes_share_legacy_bounds_and_reject_nonfinite_values() {
        let mut layout = DockLayout::default();
        let before = layout.capture_view_state();
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut saved = before.clone();
            saved.groups[1].size = value;
            assert!(!layout.restore_view_state(&saved, |_| None));
            assert_eq!(layout.capture_view_state(), before);
        }
        for value in [-100.0, f32::MAX] {
            let mut saved = before.clone();
            for group in &mut saved.groups {
                group.size = value;
            }
            assert!(layout.restore_view_state(&saved, |_| None));
            for group in layout.panels() {
                assert_eq!(group.size, panel_size(group.position, value));
            }
        }
    }

    #[test]
    fn duplicate_tree_leaves_are_rejected_without_mutation() {
        let mut layout = DockLayout::default();
        let before = layout.capture_view_state();
        let mut saved = before.clone();
        if let LayoutNodeViewState::Split { first, .. } = &mut saved.root {
            **first = LayoutNodeViewState::Tabs {
                group: "group-1".into(),
            };
        }
        assert!(!layout.restore_view_state(&saved, |_| None));
        assert_eq!(layout.capture_view_state(), before);
    }

    #[test]
    fn maximum_saved_group_id_does_not_overwrite_existing_groups() {
        let mut layout = DockLayout::default();
        let old = layout.primary_groups[&PanelPosition::Main].clone();
        let max = TabGroupId::new(u64::MAX);
        let group = layout.groups.remove(&old).unwrap();
        layout.groups.insert(max.clone(), group);
        layout
            .primary_groups
            .insert(PanelPosition::Main, max.clone());
        replace_leaf(&mut layout.root, &old, LayoutNode::Tabs(max.clone()));
        let saved = layout.capture_view_state();
        assert!(layout.restore_view_state(&saved, |_| None));
        for _ in 0..6 {
            layout
                .split_group(&max, SplitDirection::Horizontal, 0.5)
                .unwrap();
            assert!(layout.validate().is_ok());
        }
        assert_eq!(layout.groups.len(), 10);
    }

    #[test]
    fn unavailable_tabs_are_skipped_without_rejecting_valid_layout() {
        let mut layout = DockLayout::default();
        let mut saved = layout.capture_view_state();
        saved.groups[0].tabs = vec!["unknown-view".into(), "known-view".into()];
        assert!(layout.restore_view_state(&saved, |raw| {
            (raw == "known-view").then(|| PanelTab::tool("known-view"))
        }));
        assert_eq!(
            layout.panel(PanelPosition::Left).tabs,
            vec![PanelTab::tool("known-view")]
        );
    }

    #[test]
    fn anchor_identity_and_group_placement_survive_round_trip() {
        let mut layout = DockLayout::default();
        let left = layout.primary_group_id(PanelPosition::Left).clone();
        let second = layout
            .split_group(&left, SplitDirection::Horizontal, 0.5)
            .unwrap();
        layout
            .primary_groups
            .insert(PanelPosition::Left, second.clone());
        let tab = PanelTab::tool_instance("terminal", "stable-resource-view");
        layout.group_mut(&left).unwrap().activate(tab.clone());
        layout.move_tab(&left, &second, &tab).unwrap();
        let saved = layout.capture_view_state();
        let mut restored = DockLayout::default();
        assert!(restored.restore_view_state(&saved, |raw| {
            let (view, instance) = raw.strip_prefix("tool:")?.split_once(':')?;
            Some(PanelTab::tool_instance(view, instance))
        }));
        assert_eq!(restored.primary_group_id(PanelPosition::Left), &second);
        assert_eq!(restored.group_for_tab(&tab), Some(second));
        assert_eq!(restored.capture_view_state(), saved);
    }

    #[test]
    fn duplicate_instance_identity_is_rejected_even_for_different_definitions() {
        let mut layout = DockLayout::default();
        let mut saved = layout.capture_view_state();
        saved.groups[0].tabs = vec!["first".into(), "second".into()];
        let before = layout.capture_view_state();
        assert!(
            !layout.restore_view_state(&saved, |view| Some(PanelTab::tool_instance(
                view,
                "same-instance"
            )))
        );
        assert_eq!(layout.capture_view_state(), before);
    }

    #[test]
    fn invalid_ratio_and_active_placement_are_part_of_the_common_contract() {
        let mut layout = DockLayout::default();
        if let LayoutNode::Split { ratio, .. } = &mut layout.root {
            *ratio = f32::NAN;
        }
        assert_eq!(layout.validate(), Err(LayoutError::InvalidRatio));
        let mut layout = DockLayout::default();
        layout.panel_mut(PanelPosition::Left).active = Some(PanelTab::tool("missing"));
        assert_eq!(layout.validate(), Err(LayoutError::InvalidTree));
    }
}
