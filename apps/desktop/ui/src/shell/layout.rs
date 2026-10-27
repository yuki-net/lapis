//! ZoneとPanelの配置を所有する。TabはView参照でありResourceを所有しない。

use std::collections::{BTreeMap, HashSet};
use std::time::Instant;

use super::panel_layout::{Axis, PanelEdge};
use super::zone::Zone;
use super::{PanelHost, PanelId, PanelTab};
use crate::extension_ui::ZonePosition;

const POSITIONS: [ZonePosition; 4] = [
    ZonePosition::Left,
    ZonePosition::Main,
    ZonePosition::Right,
    ZonePosition::Bottom,
];

pub(crate) fn serialize_tab(tab: &PanelTab) -> String {
    match tab {
        PanelTab::Document(id) => format!("document:{}", id.as_str()),
        PanelTab::Tool { view, instance } => {
            format!("tool:{}:{}", view.as_str(), instance.as_str())
        }
    }
}

pub(crate) fn panel_position(value: &str) -> Option<ZonePosition> {
    match value {
        "main-panel" => Some(ZonePosition::Main),
        "left-panel" => Some(ZonePosition::Left),
        "bottom-panel" => Some(ZonePosition::Bottom),
        "right-panel" => Some(ZonePosition::Right),
        _ => None,
    }
}

pub(crate) fn panel_position_name(position: ZonePosition) -> &'static str {
    match position {
        ZonePosition::Main => "main-panel",
        ZonePosition::Left => "left-panel",
        ZonePosition::Bottom => "bottom-panel",
        ZonePosition::Right => "right-panel",
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DockLayout {
    zones: BTreeMap<ZonePosition, Zone>,
    next_id: u64,
    pub(super) bottom_span_left: bool,
    pub(super) bottom_span_right: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PanelDrop {
    Into(PanelId),
    IntoZone(ZonePosition),
    Edge { panel: PanelId, edge: PanelEdge },
}

impl Default for DockLayout {
    fn default() -> Self {
        let mut zones = BTreeMap::new();
        for position in POSITIONS {
            zones.insert(position, Zone::new(position));
        }
        assert!(zones.get_mut(&ZonePosition::Main).unwrap().add_panel(
            PanelId::new(1),
            Vec::new(),
            None
        ));
        let id = PanelId::new(2);
        zones.get_mut(&ZonePosition::Left).unwrap().add_panel(
            id,
            vec![PanelTab::tool(crate::features::id::VIEW_FILES)],
            None,
        );
        zones
            .get_mut(&ZonePosition::Left)
            .unwrap()
            .finish_transition_immediately();
        Self {
            zones,
            next_id: 3,
            bottom_span_left: false,
            bottom_span_right: false,
        }
    }
}

impl DockLayout {
    pub(crate) fn set_zone_open(&mut self, position: ZonePosition, open: bool) {
        self.zones
            .get_mut(&position)
            .expect("全Zoneを保持する")
            .set_open(open);
    }

    /// 明示的な境界操作だけがZoneのサイズや開閉状態を変更する。
    pub(crate) fn resize_zone(
        &mut self,
        position: ZonePosition,
        value: f32,
        maximum: f32,
        shrinking: bool,
    ) -> bool {
        let policy = super::zone_policy::ZonePolicy::for_position(position);
        if !policy.user_closable || !value.is_finite() || !maximum.is_finite() {
            return false;
        }
        if value < policy.close_threshold && shrinking {
            self.set_zone_open(position, false);
            true
        } else if value >= policy.close_threshold && maximum >= policy.interactive_min_extent {
            let other = match position {
                ZonePosition::Left => self.zone(ZonePosition::Right),
                ZonePosition::Right => self.zone(ZonePosition::Left),
                _ => self.zone(ZonePosition::Main),
            };
            let limit = if position != ZonePosition::Bottom && other.open {
                (1.0 - other.extent_ratio).max(0.0)
            } else {
                1.0
            };
            if limit <= 0.0 {
                return false;
            }
            self.zones.get_mut(&position).unwrap().extent_ratio =
                (value.clamp(policy.interactive_min_extent, maximum) / maximum).min(limit);
            true
        } else {
            false
        }
    }

    pub(crate) fn set_panel_open(&mut self, id: &PanelId, open: bool) -> bool {
        self.position_for(id).is_some_and(|position| {
            self.zones
                .get_mut(&position)
                .unwrap()
                .set_panel_open(id, open)
        })
    }

    pub(crate) fn first_panel_in_zone(&self, position: ZonePosition) -> Option<PanelId> {
        let mut ids = Vec::new();
        self.zone(position).layout.as_ref()?.panels(&mut ids);
        ids.into_iter()
            .find(|id| self.panel(id).is_some_and(|panel| panel.open))
    }

    pub(crate) fn zone_open(&self, position: ZonePosition) -> bool {
        self.zone(position).open
    }

    pub(crate) fn zone_has_panels(&self, position: ZonePosition) -> bool {
        !self.zone(position).panels.is_empty()
    }

    pub(crate) fn can_add_panel(&self, position: ZonePosition) -> bool {
        super::zone_policy::ZonePolicy::for_position(position)
            .can_add(self.zone(position).panels.len())
    }

    pub(crate) fn growth_panel(&self, position: ZonePosition) -> Option<PanelId> {
        let policy = super::zone_policy::ZonePolicy::for_position(position);
        let (_, before) = policy.growth()?;
        let mut ids = Vec::new();
        self.zone(position).layout.as_ref()?.panels(&mut ids);
        if before {
            ids.first().cloned()
        } else {
            ids.last().cloned()
        }
    }

    pub(crate) fn panel_for_tab(&self, tab: &PanelTab) -> Option<PanelId> {
        self.find_tab(tab)
    }

    pub(crate) fn panel_ids(&self) -> Vec<PanelId> {
        let mut ids = Vec::new();
        for position in POSITIONS {
            if let Some(layout) = &self.zone(position).layout {
                layout.panels(&mut ids);
            }
        }
        ids
    }

    pub(crate) fn panels(&self) -> Vec<&PanelHost> {
        self.panel_ids()
            .iter()
            .filter_map(|id| self.panel(id))
            .collect()
    }

    pub(crate) fn visible_panels(&self) -> impl Iterator<Item = &PanelHost> {
        self.zones
            .values()
            .filter(|zone| zone.open)
            .flat_map(|zone| zone.panels.values())
            .filter(|panel| panel.open)
    }

    pub(crate) fn placements(&self) -> Vec<(PanelTab, PanelId)> {
        self.zones
            .values()
            .flat_map(|zone| zone.panels.iter())
            .flat_map(|(id, panel)| panel.tabs.iter().cloned().map(move |tab| (tab, id.clone())))
            .collect()
    }
    pub(super) fn zone(&self, position: ZonePosition) -> &Zone {
        self.zones.get(&position).expect("全Zoneを保持する")
    }

    pub(crate) fn panel(&self, id: &PanelId) -> Option<&PanelHost> {
        self.zones.values().find_map(|zone| zone.panels.get(id))
    }

    fn panel_mut(&mut self, id: &PanelId) -> Option<&mut PanelHost> {
        self.zones
            .values_mut()
            .find_map(|zone| zone.panels.get_mut(id))
    }

    pub(crate) fn insert_tab_in_panel(
        &mut self,
        id: &PanelId,
        tab: PanelTab,
        keep_active: bool,
    ) -> bool {
        if self
            .placements()
            .iter()
            .any(|(existing, _)| existing.instance_id() == tab.instance_id())
        {
            return false;
        }
        let Some(panel) = self.panel_mut(id) else {
            return false;
        };
        let active = panel.active.clone();
        panel.activate_without_open(tab);
        if keep_active && active.is_some() {
            panel.active = active;
        }
        true
    }

    pub(crate) fn activate_existing_tab(&mut self, tab: &PanelTab) -> bool {
        let Some(id) = self.panel_for_tab(tab) else {
            return false;
        };
        self.panel_mut(&id).unwrap().active = Some(tab.clone());
        true
    }

    fn position_for(&self, id: &PanelId) -> Option<ZonePosition> {
        self.zones
            .iter()
            .find_map(|(position, zone)| zone.panels.contains_key(id).then_some(*position))
    }

    fn allocate_id(&mut self) -> PanelId {
        loop {
            let id = PanelId::new(self.next_id);
            self.next_id = self.next_id.wrapping_add(1);
            if self.panel(&id).is_none() {
                return id;
            }
        }
    }

    /// 空Zoneへの初回追加はPanelを生成する。空のPanelを追加先として残さない。
    pub(crate) fn open_tab(
        &mut self,
        position: ZonePosition,
        tab: PanelTab,
    ) -> Result<PanelId, ()> {
        if let Some(id) = self.find_tab(&tab) {
            self.panel_mut(&id).unwrap().activate(tab);
            let position = self.position_for(&id).expect("既存PanelはZoneに所属する");
            let zone = self.zones.get_mut(&position).unwrap();
            zone.cancel_empty_close(&id);
            zone.set_open(true);
            return Ok(id);
        }
        if self
            .zones
            .values()
            .flat_map(|zone| zone.panels.values())
            .flat_map(|panel| &panel.tabs)
            .any(|existing| existing.instance_id() == tab.instance_id())
        {
            return Err(());
        }
        if let Some(id) = self.zone(position).panels.keys().next().cloned() {
            self.panel_mut(&id).unwrap().activate(tab);
            let zone = self.zones.get_mut(&position).unwrap();
            zone.cancel_empty_close(&id);
            zone.set_open(true);
            return Ok(id);
        }
        let id = self.allocate_id();
        assert!(
            self.zones
                .get_mut(&position)
                .unwrap()
                .add_panel(id.clone(), vec![tab], None)
        );
        Ok(id)
    }

    pub(crate) fn move_tab_to_zone(
        &mut self,
        position: ZonePosition,
        tab: PanelTab,
        now: Instant,
    ) -> Result<PanelId, ()> {
        let Some(source) = self.find_tab(&tab) else {
            return self.open_tab(position, tab);
        };
        if self.position_for(&source) == Some(position) {
            self.panel_mut(&source).unwrap().activate(tab);
            self.set_panel_open(&source, true);
            return Ok(source);
        }
        let target = self
            .first_panel_in_zone(position)
            .or_else(|| self.zone(position).panels.keys().next().cloned());
        if let Some(target) = target {
            let mut candidate = self.clone();
            candidate.set_panel_open(&target, true);
            let result =
                candidate.drop_tab(&source, &tab, &PanelDrop::Into(target), (0.0, 0.0), now)?;
            *self = candidate;
            return Ok(result);
        }
        let mut candidate = self.clone();
        let source_position = candidate.position_for(&source).ok_or(())?;
        let id = candidate.allocate_id();
        if !candidate.zones.get_mut(&position).unwrap().add_panel(
            id.clone(),
            vec![tab.clone()],
            None,
        ) {
            return Err(());
        }
        candidate.panel_mut(&source).unwrap().remove(&tab);
        if candidate.panel(&source).unwrap().tabs.is_empty() {
            candidate
                .zones
                .get_mut(&source_position)
                .unwrap()
                .begin_empty_close(&source, now);
        }
        if !candidate.valid() {
            return Err(());
        }
        *self = candidate;
        Ok(id)
    }

    pub(crate) fn find_tab(&self, tab: &PanelTab) -> Option<PanelId> {
        self.zones
            .values()
            .flat_map(|zone| &zone.panels)
            .find_map(|(id, panel)| panel.contains(tab).then_some(id.clone()))
    }

    /// Previewも確定も同じ候補生成を使用し、Preview時は現在の状態に触れない。
    pub(crate) fn preview_drop(
        &self,
        source: &PanelId,
        tab: &PanelTab,
        target: &PanelDrop,
        target_zone_size: (f32, f32),
        now: Instant,
    ) -> Result<(Self, PanelId), ()> {
        let mut preview = self.clone();
        let id = preview.drop_tab(source, tab, target, target_zone_size, now)?;
        Ok((preview, id))
    }

    /// Drop候補は全操作を複製上で検査してから確定する。Resourceは変更しない。
    pub(crate) fn drop_tab(
        &mut self,
        source: &PanelId,
        tab: &PanelTab,
        target: &PanelDrop,
        target_zone_size: (f32, f32),
        now: Instant,
    ) -> Result<PanelId, ()> {
        let mut candidate = self.clone();
        if let PanelDrop::IntoZone(position) = target {
            if !candidate.zone_open(*position)
                || candidate.zone_has_panels(*position)
                || !candidate
                    .panel(source)
                    .is_some_and(|panel| panel.contains(tab))
            {
                return Err(());
            }
            let id = candidate.move_tab_to_zone(*position, tab.clone(), now)?;
            *self = candidate;
            return Ok(id);
        }
        let source_position = candidate.position_for(source).ok_or(())?;
        if !candidate
            .panel(source)
            .is_some_and(|panel| panel.contains(tab))
        {
            return Err(());
        }
        let target_id = match target {
            PanelDrop::Into(id) => id,
            PanelDrop::Edge { panel, .. } => panel,
            PanelDrop::IntoZone(_) => unreachable!(),
        };
        let target_position = candidate.position_for(target_id).ok_or(())?;
        if !candidate.zone(target_position).open
            || !candidate.panel(target_id).is_some_and(|panel| panel.open)
            || candidate
                .zone(target_position)
                .closing_since(target_id)
                .is_some()
        {
            return Err(());
        }
        if matches!(target, PanelDrop::Into(id) if id == source) {
            candidate
                .panel_mut(source)
                .unwrap()
                .activate_without_open(tab.clone());
            *self = candidate;
            return Ok(source.clone());
        }
        let destination = match target {
            PanelDrop::Into(id) => id.clone(),
            PanelDrop::Edge { panel, edge } => {
                if !candidate.can_add_panel(target_position)
                    || candidate.growth_panel(target_position).as_ref() != Some(panel)
                    || !super::panel_layout::insertion_edges(target_position).contains(edge)
                {
                    return Err(());
                }
                let (axis, before) = match edge {
                    PanelEdge::Left => (Axis::Horizontal, true),
                    PanelEdge::Right => (Axis::Horizontal, false),
                    PanelEdge::Top => (Axis::Vertical, true),
                };
                let id = candidate.allocate_id();
                if !candidate
                    .zones
                    .get_mut(&target_position)
                    .unwrap()
                    .add_panel(id.clone(), vec![tab.clone()], Some((panel, axis, before)))
                {
                    return Err(());
                }
                id
            }
            PanelDrop::IntoZone(_) => unreachable!(),
        };
        candidate.panel_mut(source).unwrap().remove(tab);
        if matches!(target, PanelDrop::Into(_)) {
            candidate
                .panel_mut(&destination)
                .unwrap()
                .activate_without_open(tab.clone());
            candidate
                .zones
                .get_mut(&target_position)
                .unwrap()
                .cancel_empty_close(&destination);
        }
        if candidate.panel(source).unwrap().tabs.is_empty() {
            candidate
                .zones
                .get_mut(&source_position)
                .unwrap()
                .begin_empty_close(source, now);
        }
        if matches!(target, PanelDrop::Edge { .. }) {
            let sizes = candidate
                .zone(target_position)
                .allocations(target_zone_size, f32::from(crate::tokens::spacing::GAP))
                .ok_or(())?;
            let minimum = super::zone_policy::PanelConstraints::COMMON.interactive_min_extent;
            if !sizes.contains_key(&destination)
                || sizes
                    .values()
                    .any(|(width, height)| *width < minimum || *height < minimum)
            {
                return Err(());
            }
        }
        if !candidate.valid() {
            return Err(());
        }
        *self = candidate;
        Ok(destination)
    }

    pub(crate) fn finish_empty_close(&mut self, id: &PanelId) -> bool {
        self.position_for(id).is_some_and(|position| {
            self.zones
                .get_mut(&position)
                .unwrap()
                .finish_empty_close(id)
        })
    }

    pub(crate) fn remove_tab(&mut self, id: &PanelId, tab: &PanelTab, now: Instant) -> bool {
        let Some(position) = self.position_for(id) else {
            return false;
        };
        if !self.panel_mut(id).is_some_and(|panel| panel.remove(tab)) {
            return false;
        }
        if self.panel(id).is_some_and(|panel| panel.tabs.is_empty()) {
            self.zones
                .get_mut(&position)
                .unwrap()
                .begin_empty_close(id, now);
        }
        true
    }

    pub(crate) fn finish_due_closes(&mut self, now: Instant) -> bool {
        let mut changed = false;
        for zone in self.zones.values_mut() {
            for id in zone.closing_ids() {
                if zone.closing_since(&id).is_some_and(|started| {
                    now.saturating_duration_since(started)
                        >= super::panel_transition::PANEL_ANIMATION_DURATION
                }) {
                    changed |= zone.finish_empty_close(&id);
                }
            }
            zone.finish_animations(now);
        }
        changed
    }

    pub(crate) fn has_closing_panels(&self) -> bool {
        self.zones
            .values()
            .any(|zone| zone.animating(Instant::now()))
    }

    /// ユーザーの境界Dragだけが比率を変え、閾値を越えた側のPanelを閉じる。
    fn resize_panel_boundary(
        &mut self,
        position: ZonePosition,
        path: &[usize],
        boundary: usize,
        next: usize,
        delta: f32,
        available: f32,
    ) -> Result<Option<PanelId>, ()> {
        let mut candidate = self.clone();
        let zone = candidate.zones.get_mut(&position).ok_or(())?;
        let policy = super::zone_policy::PanelConstraints::COMMON;
        let closing = zone.layout.as_mut().ok_or(())?.resize_boundary(
            path,
            (boundary, next),
            delta,
            available,
            policy.interactive_min_extent,
            policy.close_threshold,
        )?;
        if let Some(id) = &closing
            && !zone.set_panel_open(id, false)
        {
            return Err(());
        }
        if !candidate.valid() {
            return Err(());
        }
        *self = candidate;
        Ok(closing)
    }

    pub(crate) fn resize_between_panels(
        &mut self,
        position: ZonePosition,
        first: &PanelId,
        second: &PanelId,
        delta: f32,
        available: f32,
    ) -> Result<Option<PanelId>, ()> {
        let mut ids = Vec::new();
        self.zone(position)
            .layout
            .as_ref()
            .ok_or(())?
            .panels(&mut ids);
        let first_index = ids.iter().position(|id| id == first).ok_or(())?;
        let second_index = ids.iter().position(|id| id == second).ok_or(())?;
        if first_index >= second_index {
            return Err(());
        }
        self.resize_panel_boundary(position, &[], first_index, second_index, delta, available)
    }

    pub(crate) fn valid(&self) -> bool {
        let mut ids = HashSet::new();
        let mut instances = HashSet::new();
        POSITIONS.iter().all(|position| {
            self.zones.get(position).is_some_and(|zone| {
                zone.position == *position
                    && zone.valid()
                    && zone.panels.iter().all(|(id, panel)| {
                        ids.insert(id.clone())
                            && panel.tabs.iter().all(|tab| {
                                !tab.instance_id().as_str().is_empty()
                                    && instances.insert(tab.instance_id())
                            })
                    })
            })
        }) && self.zones.len() == POSITIONS.len()
    }

    pub(crate) fn capture(&self) -> lapis_desktop_client::ZoneLayoutViewState {
        lapis_desktop_client::ZoneLayoutViewState {
            version: lapis_desktop_client::ZONE_LAYOUT_VERSION,
            bottom_span_left: self.bottom_span_left,
            bottom_span_right: self.bottom_span_right,
            zones: POSITIONS
                .into_iter()
                .map(|position| self.zone(position).capture())
                .collect(),
        }
    }

    pub(crate) fn restore<F>(
        saved: &lapis_desktop_client::ZoneLayoutViewState,
        mut parse_tab: F,
    ) -> Option<Self>
    where
        F: FnMut(&str) -> Option<PanelTab>,
    {
        if saved.version != lapis_desktop_client::ZONE_LAYOUT_VERSION {
            return None;
        }
        let mut zones = BTreeMap::new();
        for saved_zone in &saved.zones {
            let zone = Zone::restore(saved_zone, &mut parse_tab)?;
            if zones.insert(zone.position, zone).is_some() {
                return None;
            }
        }
        let model = Self {
            zones,
            next_id: 1,
            bottom_span_left: saved.bottom_span_left,
            bottom_span_right: saved.bottom_span_right,
        };
        model.valid().then_some(model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_is_permanent_and_never_accepts_another_panel() {
        let mut layout = DockLayout::default();
        let main = layout.first_panel_in_zone(ZonePosition::Main).unwrap();
        assert!(layout.panel(&main).unwrap().tabs.is_empty());
        layout.set_zone_open(ZonePosition::Main, false);
        assert!(layout.zone_open(ZonePosition::Main));
        assert!(!layout.set_panel_open(&main, false));
        let tab = PanelTab::tool("main-document");
        layout.open_tab(ZonePosition::Main, tab.clone()).unwrap();
        let before = layout.capture();
        for edge in [PanelEdge::Left, PanelEdge::Right, PanelEdge::Top] {
            assert!(
                layout
                    .drop_tab(
                        &main,
                        &tab,
                        &PanelDrop::Edge {
                            panel: main.clone(),
                            edge
                        },
                        (1000.0, 700.0),
                        Instant::now()
                    )
                    .is_err()
            );
            assert_eq!(layout.capture(), before);
        }
        assert!(layout.remove_tab(&main, &tab, Instant::now()));
        assert_eq!(layout.first_panel_in_zone(ZonePosition::Main), Some(main));
        assert!(layout.valid());
    }

    #[test]
    fn side_zones_extend_only_from_the_growth_end_up_to_three_panels() {
        for (position, edge, before) in [
            (ZonePosition::Left, PanelEdge::Right, false),
            (ZonePosition::Right, PanelEdge::Left, true),
            (ZonePosition::Bottom, PanelEdge::Top, true),
        ] {
            let mut layout = DockLayout::default();
            let first = layout.open_tab(position, PanelTab::tool("first")).unwrap();
            for index in 2..=3 {
                let tab = PanelTab::tool(format!("item-{index}"));
                let source = layout.open_tab(position, tab.clone()).unwrap();
                let target = layout.growth_panel(position).unwrap();
                let new = layout
                    .drop_tab(
                        &source,
                        &tab,
                        &PanelDrop::Edge {
                            panel: target,
                            edge,
                        },
                        (1000.0, 1000.0),
                        Instant::now(),
                    )
                    .unwrap();
                let mut order = Vec::new();
                layout
                    .zone(position)
                    .layout
                    .as_ref()
                    .unwrap()
                    .panels(&mut order);
                assert_eq!(
                    if before { order.first() } else { order.last() },
                    Some(&new)
                );
                assert_eq!(order.len(), index);
            }
            assert!(!layout.can_add_panel(position));
            let tab = PanelTab::tool("fourth");
            let source = layout.open_tab(position, tab.clone()).unwrap();
            let target = layout.growth_panel(position).unwrap();
            let snapshot = layout.capture();
            assert!(
                layout
                    .drop_tab(
                        &source,
                        &tab,
                        &PanelDrop::Edge {
                            panel: target,
                            edge
                        },
                        (1000.0, 1000.0),
                        Instant::now()
                    )
                    .is_err()
            );
            assert_eq!(layout.capture(), snapshot);
            assert!(
                layout
                    .drop_tab(
                        &source,
                        &tab,
                        &PanelDrop::Into(first),
                        (1000.0, 1000.0),
                        Instant::now()
                    )
                    .is_ok()
            );
            assert!(layout.valid());
        }
    }

    #[test]
    fn empty_side_zone_can_reopen_and_remains_open_after_restore() {
        let mut layout = DockLayout::default();
        let source = layout.first_panel_in_zone(ZonePosition::Left).unwrap();
        let tab = PanelTab::tool(crate::features::id::VIEW_FILES);
        let now = Instant::now();
        assert!(layout.remove_tab(&source, &tab, now));
        layout.finish_due_closes(now + super::super::panel_transition::PANEL_ANIMATION_DURATION);
        assert!(!layout.zone_has_panels(ZonePosition::Left));
        layout.set_zone_open(ZonePosition::Left, true);
        assert!(layout.zone_open(ZonePosition::Left));
        let restored = DockLayout::restore(&layout.capture(), |_| None).unwrap();
        assert!(restored.zone_open(ZonePosition::Left));
        assert!(!restored.zone_has_panels(ZonePosition::Left));
        assert!(restored.valid());
    }

    #[test]
    fn tab_move_closes_empty_source_without_creating_panel() {
        let mut model = DockLayout::default();
        let main_tab = PanelTab::tool("main");
        let main = model
            .open_tab(ZonePosition::Main, main_tab.clone())
            .unwrap();
        let left = model
            .find_tab(&PanelTab::tool(crate::features::id::VIEW_FILES))
            .unwrap();
        let before = model
            .zones
            .values()
            .map(|zone| zone.panels.len())
            .sum::<usize>();
        model
            .drop_tab(
                &main,
                &main_tab,
                &PanelDrop::Into(left.clone()),
                (800.0, 600.0),
                Instant::now(),
            )
            .unwrap();
        assert_eq!(
            model
                .zones
                .values()
                .map(|zone| zone.panels.len())
                .sum::<usize>(),
            before
        );
        assert!(!model.finish_empty_close(&main));
        assert_eq!(
            model
                .zones
                .values()
                .map(|zone| zone.panels.len())
                .sum::<usize>(),
            before
        );
        assert_eq!(model.find_tab(&main_tab), Some(left));
        assert!(model.valid());
    }

    #[test]
    fn panel_edges_add_inside_the_target_zone_and_restore() {
        let mut model = DockLayout::default();
        for position in [
            ZonePosition::Left,
            ZonePosition::Right,
            ZonePosition::Bottom,
        ] {
            let first_tab = PanelTab::tool(format!("first-{position:?}"));
            let moved_tab = PanelTab::tool(format!("second-{position:?}"));
            let first = model.open_tab(position, first_tab).unwrap();
            let source = model.open_tab(position, moved_tab.clone()).unwrap();
            let edge = match position {
                ZonePosition::Left => PanelEdge::Right,
                ZonePosition::Right => PanelEdge::Left,
                ZonePosition::Bottom => PanelEdge::Top,
                ZonePosition::Main => unreachable!(),
            };
            let new_id = model
                .drop_tab(
                    &source,
                    &moved_tab,
                    &PanelDrop::Edge { panel: first, edge },
                    (800.0, 600.0),
                    Instant::now(),
                )
                .unwrap();
            assert_eq!(model.position_for(&new_id), Some(position));
            assert!(model.zone(position).panels.len() >= 2);
            assert!(model.valid());
        }
        let saved = model.capture();
        let json = serde_json::to_string(&saved).unwrap();
        let decoded = serde_json::from_str(&json).unwrap();
        let tabs = model
            .zones
            .values()
            .flat_map(|zone| zone.panels.values())
            .flat_map(|panel| panel.tabs.clone())
            .collect::<Vec<_>>();
        let restored = DockLayout::restore(&decoded, |raw| {
            tabs.iter()
                .find(|tab| super::super::layout::serialize_tab(tab) == raw)
                .cloned()
        })
        .unwrap();
        assert_eq!(restored.capture(), saved);
        assert!(restored.valid());
    }

    #[test]
    fn unavailable_space_rejects_panel_creation_without_changes() {
        let mut model = DockLayout::default();
        let tab = PanelTab::tool("extra");
        let source = model.open_tab(ZonePosition::Main, tab.clone()).unwrap();
        let target = model
            .find_tab(&PanelTab::tool(crate::features::id::VIEW_FILES))
            .unwrap();
        let before = model.capture();
        assert!(
            model
                .drop_tab(
                    &source,
                    &tab,
                    &PanelDrop::Edge {
                        panel: target,
                        edge: PanelEdge::Right
                    },
                    (50.0, 200.0),
                    Instant::now()
                )
                .is_err()
        );
        assert_eq!(model.capture(), before);
        assert_eq!(model.find_tab(&tab), Some(source));
    }

    #[test]
    fn hidden_target_is_rejected_without_changing_the_layout() {
        let mut model = DockLayout::default();
        let tab = PanelTab::tool("source");
        let source = model.open_tab(ZonePosition::Main, tab.clone()).unwrap();
        let target = model
            .find_tab(&PanelTab::tool(crate::features::id::VIEW_FILES))
            .unwrap();
        model.zones.get_mut(&ZonePosition::Left).unwrap().open = false;
        let before = model.capture();
        assert!(
            model
                .drop_tab(
                    &source,
                    &tab,
                    &PanelDrop::Into(target),
                    (400.0, 400.0),
                    Instant::now()
                )
                .is_err()
        );
        assert_eq!(model.capture(), before);
    }

    #[test]
    fn empty_main_panel_accepts_a_tab_and_remains_after_removal() {
        let mut model = DockLayout::default();
        let main = model.first_panel_in_zone(ZonePosition::Main).unwrap();
        assert!(model.panel(&main).unwrap().tabs.is_empty());
        let tab = PanelTab::tool(crate::features::id::VIEW_FILES);
        let source = model.find_tab(&tab).unwrap();
        let now = Instant::now();
        let target = model
            .drop_tab(
                &source,
                &tab,
                &PanelDrop::Into(main.clone()),
                (0.0, 0.0),
                now,
            )
            .unwrap();
        assert_eq!(model.find_tab(&tab), Some(target));
        assert!(model.zone_has_panels(ZonePosition::Main));
        assert!(model.remove_tab(&main, &tab, now));
        assert_eq!(model.first_panel_in_zone(ZonePosition::Main), Some(main));
        model.finish_due_closes(now + super::super::panel_transition::PANEL_ANIMATION_DURATION);
        assert!(!model.zone_has_panels(ZonePosition::Left));
        assert!(!model.zone_open(ZonePosition::Left));
    }

    #[test]
    fn duplicate_view_instance_is_rejected_without_changing_any_zone() {
        let mut model = DockLayout::default();
        let original = PanelTab::tool_instance("one", "shared-instance");
        let duplicate = PanelTab::tool_instance("two", "shared-instance");
        model.open_tab(ZonePosition::Main, original).unwrap();
        let before = model.capture();
        assert!(model.open_tab(ZonePosition::Right, duplicate).is_err());
        assert_eq!(model.capture(), before);
    }

    #[test]
    fn opening_an_existing_tab_reopens_its_zone() {
        let mut model = DockLayout::default();
        let tab = PanelTab::tool("saved");
        let id = model.open_tab(ZonePosition::Right, tab.clone()).unwrap();
        model
            .zones
            .get_mut(&ZonePosition::Right)
            .unwrap()
            .set_open(false);
        assert!(!model.zone(ZonePosition::Right).open);
        assert_eq!(model.open_tab(ZonePosition::Right, tab), Ok(id));
        assert!(model.zone(ZonePosition::Right).open);
    }

    #[test]
    fn moving_a_tab_into_a_closed_zone_reopens_its_existing_panel() {
        let mut model = DockLayout::default();
        let incoming = PanelTab::tool("incoming");
        let source = model
            .open_tab(ZonePosition::Main, incoming.clone())
            .unwrap();
        let target = model
            .open_tab(ZonePosition::Right, PanelTab::tool("kept"))
            .unwrap();
        assert!(model.set_panel_open(&target, false));
        let actual = model
            .move_tab_to_zone(ZonePosition::Right, incoming.clone(), Instant::now())
            .unwrap();
        assert_eq!(actual, target);
        assert!(model.zone_open(ZonePosition::Right));
        assert!(model.panel(&target).unwrap().contains(&incoming));
        assert!(model.panel(&source).unwrap().tabs.is_empty());
        assert!(model.valid());
    }

    #[test]
    fn preview_and_commit_produce_the_same_panel_and_layout() {
        let mut model = DockLayout::default();
        let tab = PanelTab::tool("preview");
        let source = model.open_tab(ZonePosition::Main, tab.clone()).unwrap();
        let panel = model
            .find_tab(&PanelTab::tool(crate::features::id::VIEW_FILES))
            .unwrap();
        let target = PanelDrop::Edge {
            panel,
            edge: PanelEdge::Right,
        };
        let before = model.capture();
        let now = Instant::now();
        let (preview, preview_id) = model
            .preview_drop(&source, &tab, &target, (400.0, 400.0), now)
            .unwrap();
        assert_eq!(model.capture(), before);
        let committed_id = model
            .drop_tab(&source, &tab, &target, (400.0, 400.0), now)
            .unwrap();
        assert_eq!(preview_id, committed_id);
        assert_eq!(preview.capture(), model.capture());
    }

    #[test]
    fn zone_and_panel_visibility_restore_independently() {
        let mut model = DockLayout {
            bottom_span_left: true,
            bottom_span_right: true,
            ..DockLayout::default()
        };
        let first_tab = PanelTab::tool("one");
        let second_tab = PanelTab::tool("two");
        let source = model
            .open_tab(ZonePosition::Right, first_tab.clone())
            .unwrap();
        model
            .open_tab(ZonePosition::Right, second_tab.clone())
            .unwrap();
        let second = model
            .drop_tab(
                &source,
                &second_tab,
                &PanelDrop::Edge {
                    panel: source.clone(),
                    edge: PanelEdge::Left,
                },
                (400.0, 400.0),
                Instant::now(),
            )
            .unwrap();
        model.panel_mut(&second).unwrap().set_open_immediate(false);
        model.zones.get_mut(&ZonePosition::Right).unwrap().open = false;
        let saved = model.capture();
        let tabs = model
            .zones
            .values()
            .flat_map(|zone| zone.panels.values())
            .flat_map(|panel| panel.tabs.clone())
            .collect::<Vec<_>>();
        let restored = DockLayout::restore(&saved, |raw| {
            tabs.iter()
                .find(|tab| super::super::layout::serialize_tab(tab) == raw)
                .cloned()
        })
        .unwrap();
        assert!(restored.bottom_span_left && restored.bottom_span_right);
        assert!(!restored.zone(ZonePosition::Right).open);
        assert!(restored.panel(&source).unwrap().open);
        assert!(!restored.panel(&second).unwrap().open);
        assert_eq!(restored.find_tab(&first_tab), Some(source));
        assert_eq!(restored.find_tab(&second_tab), Some(second));
    }

    #[test]
    fn explicit_resize_closes_only_the_shrunk_panel_and_retains_tabs() {
        let mut model = DockLayout::default();
        let tab = PanelTab::tool("resize");
        let source = model.open_tab(ZonePosition::Left, tab.clone()).unwrap();
        let new = model
            .drop_tab(
                &source,
                &tab,
                &PanelDrop::Edge {
                    panel: source.clone(),
                    edge: PanelEdge::Right,
                },
                (400.0, 300.0),
                Instant::now(),
            )
            .unwrap();
        let before = model.capture();
        assert!(
            model
                .resize_panel_boundary(ZonePosition::Left, &[], 0, 1, 0.0, 0.0)
                .is_err()
        );
        assert_eq!(model.capture(), before);
        assert_eq!(
            model.resize_panel_boundary(ZonePosition::Left, &[], 0, 1, 185.0, 400.0),
            Ok(Some(new.clone()))
        );
        assert!(!model.panel(&new).unwrap().open);
        assert_eq!(model.find_tab(&tab), Some(new));
        assert!(model.panel(&source).unwrap().open);
        assert!(model.valid());
    }

    #[test]
    fn repeated_boundary_additions_rebalance_and_remain_resizable() {
        let mut model = DockLayout::default();
        let first = model
            .open_tab(ZonePosition::Left, PanelTab::tool("one"))
            .unwrap();
        for index in 2..=3 {
            let tab = PanelTab::tool(format!("item-{index}"));
            model.open_tab(ZonePosition::Left, tab.clone()).unwrap();
            let growth = model.growth_panel(ZonePosition::Left).unwrap();
            model
                .drop_tab(
                    &first,
                    &tab,
                    &PanelDrop::Edge {
                        panel: growth,
                        edge: PanelEdge::Right,
                    },
                    (1000.0, 600.0),
                    Instant::now(),
                )
                .unwrap();
            let sizes = model
                .zone(ZonePosition::Left)
                .allocations((1000.0, 600.0), 8.0)
                .unwrap();
            assert_eq!(sizes.len(), index);
            let widths = sizes.values().map(|size| size.0).collect::<Vec<_>>();
            assert!(widths.iter().all(|width| (*width - widths[0]).abs() < 0.01));
            assert!(model.valid());
        }
        let before = model.capture();
        assert_eq!(
            model.resize_panel_boundary(ZonePosition::Left, &[], 0, 1, 20.0, 984.0),
            Ok(None)
        );
        assert_ne!(model.capture(), before);
        assert!(!model.can_add_panel(ZonePosition::Left));
        assert!(model.valid());
    }

    #[test]
    fn stale_source_and_wrong_zone_edges_leave_the_layout_unchanged() {
        let mut model = DockLayout::default();
        let tab = PanelTab::tool(crate::features::id::VIEW_FILES);
        let source = model.find_tab(&tab).unwrap();
        let before = model.capture();
        assert!(
            model
                .drop_tab(
                    &PanelId::new(999),
                    &tab,
                    &PanelDrop::Into(source.clone()),
                    (400.0, 400.0),
                    Instant::now()
                )
                .is_err()
        );
        assert!(
            model
                .drop_tab(
                    &source,
                    &tab,
                    &PanelDrop::Edge {
                        panel: source.clone(),
                        edge: PanelEdge::Left
                    },
                    (400.0, 400.0),
                    Instant::now()
                )
                .is_err()
        );
        assert_eq!(model.capture(), before);
    }

    #[test]
    fn restore_rejects_duplicate_zone_and_wrong_version() {
        let model = DockLayout::default();
        let mut saved = model.capture();
        saved.version = 1;
        assert!(DockLayout::restore(&saved, |_| None).is_none());
        saved.version = 3;
        saved.zones.push(saved.zones[0].clone());
        assert!(DockLayout::restore(&saved, |_| None).is_none());
    }

    #[test]
    fn restore_rejects_invalid_sizes_and_missing_zones() {
        let model = DockLayout::default();
        let mut saved = model.capture();
        saved.zones[0].extent_ratio = f32::NAN;
        assert!(DockLayout::restore(&saved, |_| None).is_none());
        let mut saved = model.capture();
        saved.zones.pop();
        assert!(DockLayout::restore(&saved, |_| None).is_none());
    }

    #[test]
    fn restore_rejects_panel_tree_mismatch() {
        let model = DockLayout::default();
        let mut saved = model.capture();
        let zone = saved
            .zones
            .iter_mut()
            .find(|zone| zone.position == "left-panel")
            .unwrap();
        zone.panels[0].id = "panel-999".into();
        assert!(
            DockLayout::restore(&saved, |raw| raw
                .contains(crate::features::id::VIEW_FILES)
                .then(|| PanelTab::tool(crate::features::id::VIEW_FILES)))
            .is_none()
        );
    }

    #[test]
    fn restore_rejects_duplicate_tabs_across_zones() {
        let mut model = DockLayout::default();
        let tab = PanelTab::tool("same");
        model.open_tab(ZonePosition::Main, tab.clone()).unwrap();
        model
            .open_tab(ZonePosition::Right, PanelTab::tool("right"))
            .unwrap();
        let mut saved = model.capture();
        let raw = super::serialize_tab(&tab);
        let right = saved
            .zones
            .iter_mut()
            .find(|zone| zone.position == "right-panel")
            .unwrap();
        right.panels[0].tabs = vec![raw.clone()];
        right.panels[0].active_tab = Some(raw);
        assert!(
            DockLayout::restore(&saved, |raw| if raw.contains("tool:same:") {
                Some(tab.clone())
            } else {
                Some(PanelTab::tool(crate::features::id::VIEW_FILES))
            })
            .is_none()
        );
    }
}
