//! Zone内のPanel順序と比率。ViewやResourceの寿命は扱わない。

use crate::extension_ui::ZonePosition;

use super::PanelId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum PanelLayout {
    Panel(PanelId),
    Arrangement {
        axis: Axis,
        children: Vec<PanelLayout>,
        weights: Vec<f32>,
    },
}

/// 対象のPanelに紐付く端。Zone境界の両側から所属を推測しない。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum PanelEdge {
    Left,
    Right,
    Top,
}

/// Tab Drag中だけ利用するPanel自身の端。通常のZone Resizeは変更しない。
pub(super) fn insertion_edges(position: ZonePosition) -> &'static [PanelEdge] {
    match super::zone_policy::ZonePolicy::for_position(position).growth_edge {
        Some(PanelEdge::Left) => &[PanelEdge::Left],
        Some(PanelEdge::Right) => &[PanelEdge::Right],
        Some(PanelEdge::Top) => &[PanelEdge::Top],
        None => &[],
    }
}

/// 非表示を除き、縮小可能な境界幅を差し引いて表示領域を分配する。
/// 描画のための計算であり保存された比率は変更しない。
pub(super) fn allocate(
    weights: &[f32],
    visible: &[bool],
    extent: f32,
    gap: f32,
) -> Option<Vec<f32>> {
    if weights.len() != visible.len()
        || !extent.is_finite()
        || !gap.is_finite()
        || gap < 0.0
        || weights
            .iter()
            .any(|weight| !weight.is_finite() || *weight <= 0.0)
    {
        return None;
    }
    let active = visible.iter().filter(|value| **value).count();
    let sum: f32 = weights
        .iter()
        .zip(visible)
        .filter(|(_, visible)| **visible)
        .map(|(weight, _)| weight)
        .sum();
    if !sum.is_finite() {
        return None;
    }
    let content = (extent.max(0.0) - gap * active.saturating_sub(1) as f32).max(0.0);
    Some(
        weights
            .iter()
            .zip(visible)
            .map(|(weight, visible)| {
                if *visible && sum > 0.0 {
                    content * (*weight / sum)
                } else {
                    0.0
                }
            })
            .collect(),
    )
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct EdgeCandidate {
    pub panel: PanelId,
    pub edge: PanelEdge,
    /// 境界線の始点と終点。論理pixel座標。
    pub start: (f32, f32),
    pub end: (f32, f32),
}

/// 最短距離で選び、同距離ならPanel IDと端の順序で安定させる。
pub(super) fn nearest_edge(
    candidates: &[EdgeCandidate],
    point: (f32, f32),
    hit_radius: f32,
) -> Option<&EdgeCandidate> {
    if !point.0.is_finite() || !point.1.is_finite() || !hit_radius.is_finite() || hit_radius < 0.0 {
        return None;
    }
    candidates
        .iter()
        .filter_map(|candidate| {
            let (x1, y1) = candidate.start;
            let (x2, y2) = candidate.end;
            if ![x1, y1, x2, y2].iter().all(|v| v.is_finite()) {
                return None;
            }
            let (dx, dy) = (x2 - x1, y2 - y1);
            let length_squared = dx * dx + dy * dy;
            if !length_squared.is_finite() || length_squared <= 0.0 {
                return None;
            }
            let t = (((point.0 - x1) * dx + (point.1 - y1) * dy) / length_squared).clamp(0.0, 1.0);
            let distance = (point.0 - (x1 + t * dx)).hypot(point.1 - (y1 + t * dy));
            (distance.is_finite() && distance <= hit_radius).then_some((candidate, distance))
        })
        .min_by(|(a, da), (b, db)| {
            da.total_cmp(db)
                .then_with(|| a.panel.cmp(&b.panel))
                .then_with(|| a.edge.cmp(&b.edge))
        })
        .map(|(candidate, _)| candidate)
}

impl PanelLayout {
    pub(super) fn at_path(&self, path: &[usize]) -> Option<&Self> {
        let mut node = self;
        for &index in path {
            let Self::Arrangement { children, .. } = node else {
                return None;
            };
            node = children.get(index)?;
        }
        Some(node)
    }

    pub(super) fn any_visible(&self, visible: &impl Fn(&PanelId) -> bool) -> bool {
        match self {
            Self::Panel(id) => visible(id),
            Self::Arrangement { children, .. } => {
                children.iter().any(|child| child.any_visible(visible))
            }
        }
    }

    pub(super) fn allocated_sizes(
        &self,
        size: (f32, f32),
        gap: f32,
        visible: &impl Fn(&PanelId) -> bool,
        result: &mut std::collections::BTreeMap<PanelId, (f32, f32)>,
    ) -> Option<()> {
        match self {
            Self::Panel(id) => {
                if visible(id) {
                    result.insert(id.clone(), size);
                }
            }
            Self::Arrangement {
                axis,
                children,
                weights,
            } => {
                let flags = children
                    .iter()
                    .map(|child| child.any_visible(visible))
                    .collect::<Vec<_>>();
                let extent = match axis {
                    Axis::Horizontal => size.0,
                    Axis::Vertical => size.1,
                };
                let lengths = allocate(weights, &flags, extent, gap)?;
                for (child, length) in children.iter().zip(lengths) {
                    let child_size = match axis {
                        Axis::Horizontal => (length, size.1),
                        Axis::Vertical => (size.0, length),
                    };
                    child.allocated_sizes(child_size, gap, visible, result)?;
                }
            }
        }
        Some(())
    }

    pub(super) fn panel_rect(
        &self,
        target: &PanelId,
        rect: (f32, f32, f32, f32),
        gap: f32,
        visible: &impl Fn(&PanelId) -> bool,
    ) -> Option<(f32, f32, f32, f32)> {
        match self {
            Self::Panel(id) => (id == target && visible(id)).then_some(rect),
            Self::Arrangement {
                axis,
                children,
                weights,
            } => {
                let flags = children
                    .iter()
                    .map(|child| child.any_visible(visible))
                    .collect::<Vec<_>>();
                let count = flags.iter().filter(|flag| **flag).count();
                let extent = match axis {
                    Axis::Horizontal => rect.2,
                    Axis::Vertical => rect.3,
                };
                let lengths = allocate(weights, &flags, extent, gap)?;
                let actual_gap = if count > 1 {
                    gap.min(extent.max(0.0) / (count - 1) as f32)
                } else {
                    0.0
                };
                let mut cursor = 0.0;
                let mut previous = false;
                for ((child, length), shown) in children.iter().zip(lengths).zip(flags) {
                    if !shown {
                        continue;
                    }
                    if previous {
                        cursor += actual_gap;
                    }
                    let child_rect = match axis {
                        Axis::Horizontal => (rect.0 + cursor, rect.1, length, rect.3),
                        Axis::Vertical => (rect.0, rect.1 + cursor, rect.2, length),
                    };
                    if let Some(found) = child.panel_rect(target, child_rect, gap, visible) {
                        return Some(found);
                    }
                    cursor += length;
                    previous = true;
                }
                None
            }
        }
    }
    pub(super) fn capture(&self) -> lapis_desktop_client::PanelLayoutViewState {
        match self {
            Self::Panel(id) => {
                lapis_desktop_client::PanelLayoutViewState::Panel { id: id.as_str() }
            }
            Self::Arrangement {
                axis,
                children,
                weights,
            } => lapis_desktop_client::PanelLayoutViewState::Arrangement {
                direction: match axis {
                    Axis::Horizontal => "horizontal",
                    Axis::Vertical => "vertical",
                }
                .to_owned(),
                children: children.iter().map(Self::capture).collect(),
                weights: weights.clone(),
            },
        }
    }

    pub(super) fn restore(saved: &lapis_desktop_client::PanelLayoutViewState) -> Option<Self> {
        match saved {
            lapis_desktop_client::PanelLayoutViewState::Panel { id } => {
                Some(Self::Panel(PanelId::parse(id)?))
            }
            lapis_desktop_client::PanelLayoutViewState::Arrangement {
                direction,
                children,
                weights,
            } => Some(Self::Arrangement {
                axis: match direction.as_str() {
                    "horizontal" => Axis::Horizontal,
                    "vertical" => Axis::Vertical,
                    _ => return None,
                },
                children: children
                    .iter()
                    .map(Self::restore)
                    .collect::<Option<Vec<_>>>()?,
                weights: weights.clone(),
            }),
        }
    }
    /// 明示Resizeだけがclose候補を返す。Windowの自動配分からは呼ばない。
    pub(super) fn resize_boundary(
        &mut self,
        path: &[usize],
        pair: (usize, usize),
        delta: f32,
        available: f32,
        min_size: f32,
        close_threshold: f32,
    ) -> Result<Option<PanelId>, ()> {
        if !self.valid()
            || ![delta, available, min_size, close_threshold]
                .iter()
                .all(|v| v.is_finite())
            || available <= 0.0
            || min_size <= 0.0
            || close_threshold < 0.0
        {
            return Err(());
        }
        let (boundary, next) = pair;
        let mut node = self;
        for &index in path {
            let Self::Arrangement { children, .. } = node else {
                return Err(());
            };
            node = children.get_mut(index).ok_or(())?;
        }
        let Self::Arrangement {
            children, weights, ..
        } = node
        else {
            return Err(());
        };
        if next <= boundary || next >= children.len() || weights.len() != children.len() {
            return Err(());
        }
        let first = weights[boundary] * available + delta;
        let second = weights[next] * available - delta;
        if !first.is_finite() || !second.is_finite() {
            return Err(());
        }
        if delta < 0.0
            && first < close_threshold
            && let Self::Panel(id) = &children[boundary]
        {
            return Ok(Some(id.clone()));
        }
        if delta > 0.0
            && second < close_threshold
            && let Self::Panel(id) = &children[next]
        {
            return Ok(Some(id.clone()));
        }
        let combined = first + second;
        if !combined.is_finite() || combined < 2.0 * min_size {
            return Err(());
        }
        let first = first.clamp(min_size, combined - min_size);
        weights[boundary] = first / available;
        weights[next] = (combined - first) / available;
        Ok(None)
    }
    pub(super) fn panels(&self, result: &mut Vec<PanelId>) {
        match self {
            Self::Panel(id) => result.push(id.clone()),
            Self::Arrangement { children, .. } => {
                for child in children {
                    child.panels(result);
                }
            }
        }
    }

    /// 指定Panelの端へ追加する。同方向の並びは一つの配列で保持する。
    pub(super) fn insert_at_edge(
        &mut self,
        panel: &PanelId,
        new_panel: PanelId,
        axis: Axis,
        before: bool,
    ) -> bool {
        match self {
            Self::Panel(id) if id == panel => {
                let old = Self::Panel(id.clone());
                let new = Self::Panel(new_panel);
                *self = Self::Arrangement {
                    axis,
                    children: if before {
                        vec![new, old]
                    } else {
                        vec![old, new]
                    },
                    weights: vec![0.5, 0.5],
                };
                true
            }
            Self::Panel(_) => false,
            Self::Arrangement {
                axis: current,
                children,
                weights,
            } => {
                if *current == axis
                    && let Some(index) = children
                        .iter()
                        .position(|child| matches!(child, Self::Panel(id) if id == panel))
                {
                    children.insert(index + usize::from(!before), Self::Panel(new_panel));
                    *weights = vec![1.0 / children.len() as f32; children.len()];
                    return true;
                }
                children
                    .iter_mut()
                    .any(|child| child.insert_at_edge(panel, new_panel.clone(), axis, before))
            }
        }
    }

    /// 空になったPanelを閉じ、残り一つなら内部の配置を畳む。
    pub(super) fn remove(&mut self, panel: &PanelId) -> bool {
        let Self::Arrangement {
            children, weights, ..
        } = self
        else {
            return false;
        };
        if let Some(index) = children
            .iter()
            .position(|child| matches!(child, Self::Panel(id) if id == panel))
        {
            children.remove(index);
            weights.remove(index);
            if children.len() == 1 {
                *self = children.remove(0);
            } else {
                let sum: f32 = weights.iter().sum();
                if sum > 0.0 {
                    for weight in weights {
                        *weight /= sum;
                    }
                }
            }
            return true;
        }
        children.iter_mut().any(|child| child.remove(panel))
    }

    pub(super) fn valid(&self) -> bool {
        fn check(node: &PanelLayout, seen: &mut std::collections::BTreeSet<PanelId>) -> bool {
            match node {
                PanelLayout::Panel(id) => seen.insert(id.clone()),
                PanelLayout::Arrangement {
                    children, weights, ..
                } => {
                    children.len() >= 2
                        && children.len() == weights.len()
                        && weights
                            .iter()
                            .all(|value| value.is_finite() && *value > 0.0)
                        && (weights.iter().sum::<f32>() - 1.0).abs() < 0.0001
                        && children.iter().all(|child| check(child, seen))
                }
            }
        }
        check(self, &mut std::collections::BTreeSet::new())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{PanelTab, zone::Zone};
    use super::*;

    #[test]
    fn zone_closes_only_empty_panels_after_the_transition() {
        let mut zone = Zone::new(ZonePosition::Left);
        let a = PanelId::new(1);
        let b = PanelId::new(2);
        let tab_a = PanelTab::tool("a");
        let tab_b = PanelTab::tool("b");
        assert!(zone.add_panel(a.clone(), vec![tab_a.clone()], None));
        assert!(zone.add_panel(b.clone(), vec![tab_b], Some((&a, Axis::Horizontal, false))));
        assert!(!zone.begin_empty_close(&a, std::time::Instant::now()));
        assert!(zone.panels.get_mut(&a).unwrap().remove(&tab_a));
        let now = std::time::Instant::now();
        assert!(zone.begin_empty_close(&a, now));
        assert_eq!(zone.closing_since(&a), Some(now));
        assert!(zone.valid());
        assert!(zone.finish_empty_close(&a));
        assert!(zone.valid());
        assert_eq!(zone.layout, Some(PanelLayout::Panel(b)));
        assert!(zone.open);
    }

    #[test]
    fn reopening_during_empty_close_keeps_the_panel() {
        let mut zone = Zone::new(ZonePosition::Right);
        let id = PanelId::new(1);
        let tab = PanelTab::tool("a");
        assert!(zone.add_panel(id.clone(), vec![tab.clone()], None));
        assert!(zone.panels.get_mut(&id).unwrap().remove(&tab));
        assert!(zone.begin_empty_close(&id, std::time::Instant::now()));
        zone.panels.get_mut(&id).unwrap().activate_without_open(tab);
        assert!(zone.set_panel_open(&id, true));
        assert!(!zone.finish_empty_close(&id));
        assert!(zone.valid());
        assert!(zone.open);
    }

    #[test]
    fn zone_open_close_preserves_individual_panel_visibility() {
        let mut zone = Zone::new(ZonePosition::Right);
        let a = PanelId::new(1);
        let b = PanelId::new(2);
        assert!(zone.add_panel(a.clone(), vec![PanelTab::tool("a")], None));
        assert!(zone.add_panel(
            b.clone(),
            vec![PanelTab::tool("b")],
            Some((&a, Axis::Horizontal, true))
        ));
        assert!(zone.set_panel_open(&b, false));
        zone.set_open(false);
        assert!(!zone.open);
        assert!(zone.panels[&a].open);
        assert!(!zone.panels[&b].open);
        zone.set_open(true);
        assert!(zone.open);
        assert!(zone.set_panel_open(&a, false));
        assert!(!zone.open);
        assert!(zone.set_panel_open(&b, true));
        assert!(zone.open);
    }

    #[test]
    fn zone_close_and_reopen_animate_without_discarding_panel_state() {
        let mut zone = Zone::new(ZonePosition::Left);
        let id = PanelId::new(1);
        assert!(zone.add_panel(id.clone(), vec![PanelTab::tool("a")], None));
        zone.finish_transition_immediately();
        let start = std::time::Instant::now();
        zone.set_open(false);
        let middle = zone.visual_factor(start + std::time::Duration::from_millis(60));
        assert!(middle > 0.0 && middle < 1.0);
        assert_eq!(
            zone.visual_factor(start + std::time::Duration::from_millis(125)),
            0.0
        );
        assert!(zone.panels.contains_key(&id));
        zone.set_open(true);
        assert!(zone.open);
        assert!(zone.panels[&id].open);
    }

    #[test]
    fn zone_round_trip_keeps_panel_identity_and_omits_closing_empty_panel() {
        let mut zone = Zone::new(ZonePosition::Left);
        let first = PanelId::new(1);
        let second = PanelId::new(2);
        let a = PanelTab::tool("a");
        let b = PanelTab::tool("b");
        assert!(zone.add_panel(first.clone(), vec![a.clone()], None));
        assert!(zone.add_panel(
            second.clone(),
            vec![b.clone()],
            Some((&first, Axis::Horizontal, false))
        ));
        let saved = zone.capture();
        let json = serde_json::to_string(&saved).unwrap();
        let decoded = serde_json::from_str(&json).unwrap();
        let mut parse = |raw: &str| {
            if raw.contains("tool:a:") {
                Some(a.clone())
            } else if raw.contains("tool:b:") {
                Some(b.clone())
            } else {
                None
            }
        };
        let restored = Zone::restore(&decoded, &mut parse).unwrap();
        assert_eq!(restored.capture(), saved);
        assert!(zone.panels.get_mut(&first).unwrap().remove(&a));
        assert!(zone.begin_empty_close(&first, std::time::Instant::now()));
        let saved = zone.capture();
        assert_eq!(saved.panels.len(), 1);
        assert_eq!(saved.panels[0].id, second.as_str());
        assert!(Zone::restore(&saved, &mut parse).unwrap().valid());
    }

    #[test]
    fn edge_insertion_balances_multiple_panels_and_removal_collapses() {
        let mut layout = PanelLayout::Panel(PanelId::new(1));
        assert!(layout.insert_at_edge(&PanelId::new(1), PanelId::new(2), Axis::Horizontal, false));
        assert!(layout.insert_at_edge(&PanelId::new(2), PanelId::new(3), Axis::Horizontal, true));
        assert!(layout.insert_at_edge(&PanelId::new(3), PanelId::new(4), Axis::Horizontal, false));
        let mut ids = Vec::new();
        layout.panels(&mut ids);
        assert_eq!(
            ids,
            [1, 3, 4, 2]
                .into_iter()
                .map(PanelId::new)
                .collect::<Vec<_>>()
        );
        assert!(layout.valid());
        for id in [2, 3, 4] {
            assert!(layout.remove(&PanelId::new(id)));
        }
        assert_eq!(layout, PanelLayout::Panel(PanelId::new(1)));
    }

    #[test]
    fn independent_axes_keep_a_valid_arrangement() {
        let mut layout = PanelLayout::Panel(PanelId::new(1));
        assert!(layout.insert_at_edge(&PanelId::new(1), PanelId::new(2), Axis::Horizontal, false));
        assert!(layout.insert_at_edge(&PanelId::new(2), PanelId::new(3), Axis::Vertical, true));
        assert!(layout.valid());
        assert!(layout.remove(&PanelId::new(2)));
        assert!(layout.valid());
    }

    #[test]
    fn nearest_boundary_uses_distance_and_stable_tie_break() {
        let candidates = vec![
            EdgeCandidate {
                panel: PanelId::new(2),
                edge: PanelEdge::Right,
                start: (10.0, 0.0),
                end: (10.0, 100.0),
            },
            EdgeCandidate {
                panel: PanelId::new(1),
                edge: PanelEdge::Top,
                start: (0.0, 10.0),
                end: (100.0, 10.0),
            },
        ];
        assert_eq!(
            nearest_edge(&candidates, (11.0, 30.0), 20.0).unwrap().panel,
            PanelId::new(2)
        );
        assert_eq!(
            nearest_edge(&candidates, (10.0, 10.0), 20.0).unwrap().panel,
            PanelId::new(1)
        );
        assert!(nearest_edge(&candidates, (80.0, 80.0), 20.0).is_none());
        assert!(nearest_edge(&candidates, (f32::NAN, 0.0), 20.0).is_none());
    }

    #[test]
    fn insertion_edges_are_bound_to_the_panel_zone() {
        assert_eq!(insertion_edges(ZonePosition::Left), &[PanelEdge::Right]);
        assert_eq!(insertion_edges(ZonePosition::Right), &[PanelEdge::Left]);
        assert!(insertion_edges(ZonePosition::Main).is_empty());
        assert_eq!(insertion_edges(ZonePosition::Bottom), &[PanelEdge::Top]);
    }

    #[test]
    fn window_shrink_and_visibility_never_change_saved_weights() {
        let weights = [0.2, 0.3, 0.5];
        assert_eq!(
            allocate(&weights, &[true, true, true], 2.0, 8.0),
            Some(vec![0.0, 0.0, 0.0])
        );
        let expanded = allocate(&weights, &[true, false, true], 708.0, 8.0).unwrap();
        assert!((expanded[0] - 200.0).abs() < 0.001);
        assert_eq!(expanded[1], 0.0);
        assert!((expanded[2] - 500.0).abs() < 0.001);
        assert_eq!(weights, [0.2, 0.3, 0.5]);
        assert!(allocate(&weights, &[true, true, true], f32::NAN, 8.0).is_none());
    }

    #[test]
    fn preview_rect_follows_gap_visibility_and_panel_weights() {
        let layout = PanelLayout::Arrangement {
            axis: Axis::Horizontal,
            children: vec![
                PanelLayout::Panel(PanelId::new(1)),
                PanelLayout::Panel(PanelId::new(2)),
                PanelLayout::Panel(PanelId::new(3)),
            ],
            weights: vec![0.25, 0.25, 0.5],
        };
        let visible = |id: &PanelId| *id != PanelId::new(2);
        let rect = layout
            .panel_rect(&PanelId::new(3), (0.0, 0.0, 308.0, 200.0), 8.0, &visible)
            .unwrap();
        assert!((rect.0 - 108.0).abs() < 0.001);
        assert!((rect.2 - 200.0).abs() < 0.001);
        assert!(
            layout
                .panel_rect(&PanelId::new(2), (0.0, 0.0, 308.0, 200.0), 8.0, &visible)
                .is_none()
        );
    }

    #[test]
    fn close_threshold_is_only_evaluated_for_explicit_resize() {
        let mut layout = PanelLayout::Panel(PanelId::new(1));
        assert!(layout.insert_at_edge(&PanelId::new(1), PanelId::new(2), Axis::Horizontal, false));
        let before = layout.clone();
        assert!(
            layout
                .resize_boundary(&[], (0, 1), 0.0, 0.0, 24.0, 12.0)
                .is_err()
        );
        assert_eq!(layout, before);
        assert!(
            layout
                .resize_boundary(&[], (0, 1), 0.0, 20.0, 24.0, 12.0)
                .is_err()
        );
        assert_eq!(layout, before);
        assert_eq!(
            layout.resize_boundary(&[], (0, 1), 1.0, 20.0, 24.0, 12.0),
            Ok(Some(PanelId::new(2)))
        );
        assert_eq!(layout, before);
        assert_eq!(
            layout.resize_boundary(&[], (0, 1), -95.0, 200.0, 24.0, 12.0),
            Ok(Some(PanelId::new(1)))
        );
        assert_eq!(layout, before);
        assert_eq!(
            layout.resize_boundary(&[], (0, 1), 30.0, 200.0, 24.0, 12.0),
            Ok(None)
        );
        assert!(layout.valid());
    }

    #[test]
    fn duplicate_panel_ids_and_invalid_weights_are_rejected() {
        let mut layout = PanelLayout::Arrangement {
            axis: Axis::Horizontal,
            children: vec![
                PanelLayout::Panel(PanelId::new(1)),
                PanelLayout::Panel(PanelId::new(1)),
            ],
            weights: vec![0.5, 0.5],
        };
        assert!(!layout.valid());
        assert!(
            layout
                .resize_boundary(&[], (0, 1), 1.0, 200.0, 24.0, 12.0)
                .is_err()
        );
        if let PanelLayout::Arrangement {
            children, weights, ..
        } = &mut layout
        {
            children[1] = PanelLayout::Panel(PanelId::new(2));
            weights[0] = f32::NAN;
        }
        assert!(!layout.valid());
        assert!(
            layout
                .resize_boundary(&[], (0, 1), 1.0, 200.0, 24.0, 12.0)
                .is_err()
        );
    }
}
