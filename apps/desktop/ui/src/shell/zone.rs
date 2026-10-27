//! Zoneの表示状態とPanelを管理する。

use super::panel_layout::{Axis, PanelLayout};
use super::{PanelHost, PanelTab, TabGroupId};
use crate::extension_ui::PanelPosition;

/// ZoneはPanelの並びと表示状態を所有する。空Panelの閉鎖中だけ一時的に残す。
#[derive(Clone, Debug)]
pub(super) struct Zone {
    pub(super) position: PanelPosition,
    pub(super) layout: Option<PanelLayout>,
    pub(super) panels: std::collections::BTreeMap<TabGroupId, PanelHost>,
    pub(super) open: bool,
    pub(super) size: f32,
    closing: std::collections::BTreeMap<TabGroupId, std::time::Instant>,
    hiding: std::collections::BTreeMap<TabGroupId, std::time::Instant>,
    transition: Option<(std::time::Instant, f32, f32)>,
}

impl Zone {
    pub(super) fn new(position: PanelPosition, size: f32) -> Self {
        Self {
            position,
            layout: None,
            panels: Default::default(),
            open: position == PanelPosition::Main,
            size,
            closing: Default::default(),
            hiding: Default::default(),
            transition: None,
        }
    }

    /// Zoneの表示切替は各Panelの個別表示状態を変更しない。
    pub(super) fn set_open(&mut self, open: bool) {
        let open = open
            && (self.position == PanelPosition::Main
                || self.panels.values().any(|panel| panel.open));
        if self.open != open {
            let now = std::time::Instant::now();
            let from = self.visual_factor(now);
            self.open = open;
            self.transition = Some((now, from, if open { 1.0 } else { 0.0 }));
        }
    }

    pub(super) fn finish_transition_immediately(&mut self) {
        self.transition = None;
    }

    pub(super) fn visual_factor(&self, now: std::time::Instant) -> f32 {
        self.transition
            .map_or(if self.open { 1.0 } else { 0.0 }, |(started, from, to)| {
                let elapsed = now.saturating_duration_since(started).as_secs_f32();
                let progress = (elapsed
                    / super::panel_transition::PANEL_ANIMATION_DURATION.as_secs_f32())
                .clamp(0.0, 1.0);
                let eased = 1.0 - (1.0 - progress).powi(3);
                from + (to - from) * eased
            })
    }

    pub(super) fn animating(&self, now: std::time::Instant) -> bool {
        self.transition.is_some_and(|(start, _, _)| {
            now.saturating_duration_since(start) < super::panel_transition::PANEL_ANIMATION_DURATION
        }) || !self.closing.is_empty()
            || !self.hiding.is_empty()
    }

    pub(super) fn hiding_since(&self, id: &TabGroupId) -> Option<std::time::Instant> {
        self.hiding.get(id).copied()
    }

    pub(super) fn set_panel_open(&mut self, id: &TabGroupId, open: bool) -> bool {
        let Some(panel) = self.panels.get_mut(id) else {
            return false;
        };
        let was_open = panel.open;
        panel.set_open_immediate(open);
        if open {
            self.closing.remove(id);
            self.hiding.remove(id);
        } else if was_open {
            self.hiding
                .entry(id.clone())
                .or_insert_with(std::time::Instant::now);
        }
        if open {
            self.set_open(true);
        } else if self.position != PanelPosition::Main
            && self.panels.values().all(|panel| !panel.open)
        {
            self.set_open(false);
        }
        true
    }

    /// 初回のPanelはZoneに作成し、既存Panelの端への追加は同じZone内に限定する。
    pub(super) fn add_panel(
        &mut self,
        id: TabGroupId,
        tabs: Vec<PanelTab>,
        at: Option<(&TabGroupId, Axis, bool)>,
    ) -> bool {
        if tabs.is_empty() || self.panels.contains_key(&id) {
            return false;
        }
        match (&mut self.layout, at) {
            (None, None) => self.layout = Some(PanelLayout::Panel(id.clone())),
            (Some(layout), Some((target, axis, before))) => {
                if !layout.insert_at_edge(target, id.clone(), axis, before) {
                    return false;
                }
            }
            _ => return false,
        }
        self.panels
            .insert(id, PanelHost::new(self.position, tabs, true));
        self.set_open(true);
        true
    }

    /// 最後のTabの移動後は閉鎖中として保持し、描画側が遷移終了後に除去する。
    pub(super) fn begin_empty_close(&mut self, id: &TabGroupId, now: std::time::Instant) -> bool {
        if !self
            .panels
            .get(id)
            .is_some_and(|panel| panel.tabs.is_empty())
        {
            return false;
        }
        self.closing.entry(id.clone()).or_insert(now);
        if self.position != PanelPosition::Main
            && self.panels.iter().all(|(panel_id, panel)| {
                panel_id == id || !panel.open || self.closing.contains_key(panel_id)
            })
        {
            self.set_open(false);
        }
        true
    }

    pub(super) fn closing_since(&self, id: &TabGroupId) -> Option<std::time::Instant> {
        self.closing.get(id).copied()
    }

    pub(super) fn available_for_resize(
        &self,
        path: &[usize],
        extent: f32,
        gap: f32,
    ) -> Option<f32> {
        if !extent.is_finite() || !gap.is_finite() || extent <= 0.0 || gap < 0.0 {
            return None;
        }
        let PanelLayout::Arrangement {
            children, weights, ..
        } = self.layout.as_ref()?.at_path(path)?
        else {
            return None;
        };
        let flags = children
            .iter()
            .map(|child| {
                child.any_visible(&|id| self.panels.get(id).is_some_and(|panel| panel.open))
            })
            .collect::<Vec<_>>();
        let visible = flags.iter().filter(|visible| **visible).count();
        let sum = weights
            .iter()
            .zip(flags)
            .filter_map(|(weight, visible)| visible.then_some(*weight))
            .sum::<f32>();
        let result = extent - gap * visible.saturating_sub(1) as f32;
        (result > 0.0 && sum.is_finite() && sum > 0.0).then_some(result / sum)
    }

    pub(super) fn cancel_empty_close(&mut self, id: &TabGroupId) {
        self.closing.remove(id);
    }

    pub(super) fn closing_ids(&self) -> Vec<TabGroupId> {
        self.closing.keys().cloned().collect()
    }

    pub(super) fn finish_animations(&mut self, now: std::time::Instant) {
        let duration = super::panel_transition::PANEL_ANIMATION_DURATION;
        self.hiding
            .retain(|_, started| now.saturating_duration_since(*started) < duration);
        if self
            .transition
            .is_some_and(|(started, _, _)| now.saturating_duration_since(started) >= duration)
        {
            self.transition = None;
        }
    }

    /// GPUIのflex配分と同じ比率で、追加候補の最小サイズを検査する。
    pub(super) fn allocations(
        &self,
        size: (f32, f32),
        gap: f32,
    ) -> Option<std::collections::BTreeMap<TabGroupId, (f32, f32)>> {
        let mut result = std::collections::BTreeMap::new();
        if !size.0.is_finite() || !size.1.is_finite() || size.0 < 0.0 || size.1 < 0.0 {
            return None;
        }
        if let Some(layout) = &self.layout {
            layout.allocated_sizes(
                size,
                gap,
                &|id| {
                    self.open
                        && !self.closing.contains_key(id)
                        && self.panels.get(id).is_some_and(|panel| panel.open)
                },
                &mut result,
            )?;
        }
        Some(result)
    }

    pub(super) fn panel_rect(
        &self,
        id: &TabGroupId,
        size: (f32, f32),
        gap: f32,
    ) -> Option<(f32, f32, f32, f32)> {
        if !size.0.is_finite() || !size.1.is_finite() || size.0 < 0.0 || size.1 < 0.0 {
            return None;
        }
        self.layout
            .as_ref()?
            .panel_rect(id, (0.0, 0.0, size.0, size.1), gap, &|id| {
                self.open
                    && !self.closing.contains_key(id)
                    && self.panels.get(id).is_some_and(|panel| panel.open)
            })
    }

    /// 閉鎖中にTabが再追加された場合はそのPanelを保持する。
    pub(super) fn finish_empty_close(&mut self, id: &TabGroupId) -> bool {
        if !self.closing.contains_key(id) {
            return false;
        }
        if !self
            .panels
            .get(id)
            .is_some_and(|panel| panel.tabs.is_empty())
        {
            self.closing.remove(id);
            return false;
        }
        if matches!(&self.layout, Some(PanelLayout::Panel(current)) if current == id) {
            self.layout = None;
        } else if !self.layout.as_mut().is_some_and(|layout| layout.remove(id)) {
            return false;
        }
        self.closing.remove(id);
        self.hiding.remove(id);
        self.panels.remove(id);
        if self.panels.values().all(|panel| !panel.open) && self.position != PanelPosition::Main {
            self.set_open(false);
        }
        true
    }

    pub(super) fn valid(&self) -> bool {
        if !self.size.is_finite() || self.size < 0.0 {
            return false;
        }
        let mut ids = Vec::new();
        if let Some(layout) = &self.layout {
            if !layout.valid() {
                return false;
            }
            layout.panels(&mut ids);
        }
        ids.len() == self.panels.len()
            && ids.iter().all(|id| {
                self.panels.get(id).is_some_and(|panel| {
                    panel.position == self.position
                        && (!panel.tabs.is_empty() || self.closing.contains_key(id))
                })
            })
            && self.closing.keys().all(|id| self.panels.contains_key(id))
    }

    /// 遷移中の空Panelは永続状態から除く。閉鎖中の見た目は実行時状態のみ。
    pub(super) fn capture(&self) -> lapis_desktop_client::ZoneViewState {
        let mut stable = self.clone();
        let closing = stable.closing.keys().cloned().collect::<Vec<_>>();
        for id in closing {
            stable.finish_empty_close(&id);
        }
        lapis_desktop_client::ZoneViewState {
            position: super::layout::panel_position_name(stable.position).to_owned(),
            open: stable.open,
            size: stable.size,
            layout: stable.layout.as_ref().map(PanelLayout::capture),
            panels: stable
                .panels
                .iter()
                .map(|(id, panel)| lapis_desktop_client::ZonePanelViewState {
                    id: id.as_str(),
                    tabs: panel
                        .tabs
                        .iter()
                        .map(super::layout::serialize_tab)
                        .collect(),
                    active_tab: panel.active.as_ref().map(super::layout::serialize_tab),
                    open: panel.requested_open(),
                })
                .collect(),
        }
    }

    /// 不正な配置は候補全体を拒否し、利用できないTabだけを取り除く。
    pub(super) fn restore<F>(
        saved: &lapis_desktop_client::ZoneViewState,
        parse_tab: &mut F,
    ) -> Option<Self>
    where
        F: FnMut(&str) -> Option<PanelTab>,
    {
        let position = super::layout::panel_position(&saved.position)?;
        if super::layout::panel_position_name(position) != saved.position
            || !saved.size.is_finite()
            || saved.size < 0.0
        {
            return None;
        }
        let layout = match &saved.layout {
            Some(layout) => Some(PanelLayout::restore(layout)?),
            None => None,
        };
        let mut panels = std::collections::BTreeMap::new();
        let mut seen = std::collections::HashSet::new();
        for item in &saved.panels {
            let id = TabGroupId::parse(&item.id)?;
            let mut tabs = Vec::new();
            for raw in &item.tabs {
                let Some(tab) = parse_tab(raw) else { continue };
                if !seen.insert(tab.instance_id()) {
                    return None;
                }
                tabs.push(tab);
            }
            let active = item
                .active_tab
                .as_deref()
                .and_then(&mut *parse_tab)
                .filter(|tab| tabs.contains(tab))
                .or_else(|| tabs.first().cloned());
            let mut panel = PanelHost::new(position, tabs, item.open);
            panel.active = active;
            panel.set_open_immediate(item.open);
            if panels.insert(id, panel).is_some() {
                return None;
            }
        }
        let mut zone = Self {
            position,
            layout,
            panels,
            open: saved.open,
            size: saved.size,
            closing: Default::default(),
            hiding: Default::default(),
            transition: None,
        };
        let mut ids = Vec::new();
        if let Some(layout) = &zone.layout {
            if !layout.valid() {
                return None;
            }
            layout.panels(&mut ids);
        }
        if ids.len() != zone.panels.len() || ids.iter().any(|id| !zone.panels.contains_key(id)) {
            return None;
        }
        let empty = zone
            .panels
            .iter()
            .filter(|(_, panel)| panel.tabs.is_empty())
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in empty {
            zone.begin_empty_close(&id, std::time::Instant::now());
            zone.finish_empty_close(&id);
        }
        if zone.position != PanelPosition::Main && zone.panels.values().all(|panel| !panel.open) {
            zone.open = false;
        }
        zone.valid().then_some(zone)
    }
}
