//! 保存済みの比率とWindow寸法から、描画・操作で共有する矩形を計算する。

use std::collections::BTreeMap;
use std::time::Instant;

use super::PanelDrop;
use super::panel_layout::Axis;
use super::panel_layout::{EdgeCandidate, PanelEdge, insertion_edges, nearest_edge};
use super::{DockLayout, PanelId};
use crate::extension_ui::ZonePosition;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct DockRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl DockRect {
    pub fn offset(self, x: f32, y: f32) -> Self {
        Self {
            x: self.x + x,
            y: self.y + y,
            ..self
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DockGeometry {
    pub frame: DockRect,
    pub zones: BTreeMap<ZonePosition, DockRect>,
    pub panels: BTreeMap<PanelId, DockRect>,
    pub panel_boundaries: Vec<PanelBoundary>,
    pub gap: f32,
}

#[derive(Clone, Debug)]
pub(crate) struct PanelBoundary {
    pub zone: ZonePosition,
    pub first: PanelId,
    pub second: PanelId,
    pub axis: Axis,
    pub rect: DockRect,
    pub available: f32,
}

impl DockGeometry {
    pub fn drop_target(
        layout: &DockLayout,
        panels: impl Iterator<Item = (PanelId, DockRect)>,
        zones: impl Iterator<Item = (ZonePosition, DockRect)>,
        point: (f32, f32),
    ) -> Option<PanelDrop> {
        let mut edges = Vec::new();
        let mut faces = Vec::new();
        for (id, rect) in panels {
            let Some(panel) = layout.panel(&id) else {
                continue;
            };
            if !panel.open
                || !layout.zone_open(panel.position)
                || rect.width <= 0.0
                || rect.height <= 0.0
            {
                continue;
            }
            if layout.can_add_panel(panel.position)
                && layout.growth_panel(panel.position).as_ref() == Some(&id)
            {
                for &edge in insertion_edges(panel.position) {
                    let (start, end) = match edge {
                        PanelEdge::Left => ((rect.x, rect.y), (rect.x, rect.y + rect.height)),
                        PanelEdge::Right => (
                            (rect.x + rect.width, rect.y),
                            (rect.x + rect.width, rect.y + rect.height),
                        ),
                        PanelEdge::Top => ((rect.x, rect.y), (rect.x + rect.width, rect.y)),
                    };
                    edges.push(EdgeCandidate {
                        panel: id.clone(),
                        edge,
                        start,
                        end,
                    });
                }
            }
            if point.0 >= rect.x
                && point.1 >= rect.y
                && point.0 < rect.x + rect.width
                && point.1 < rect.y + rect.height
            {
                faces.push(id);
            }
        }
        faces.sort();
        nearest_edge(&edges, point, f32::from(crate::tokens::spacing::GAP) * 2.5)
            .map(|candidate| PanelDrop::Edge {
                panel: candidate.panel.clone(),
                edge: candidate.edge,
            })
            .or_else(|| faces.into_iter().next().map(PanelDrop::Into))
            .or_else(|| {
                zones
                    .filter(|(position, rect)| {
                        layout.zone_open(*position)
                            && !layout.zone_has_panels(*position)
                            && point.0 >= rect.x
                            && point.1 >= rect.y
                            && point.0 < rect.x + rect.width
                            && point.1 < rect.y + rect.height
                    })
                    .map(|(position, _)| PanelDrop::IntoZone(position))
                    .next()
            })
    }

    /// Window縮小はこの値だけを変え、保存比率や開閉状態は更新しない。
    pub fn calculate(
        layout: &DockLayout,
        viewport: (f32, f32),
        detached: bool,
        spans: (bool, bool),
        now: Instant,
    ) -> Self {
        let width = if viewport.0.is_finite() {
            viewport.0.max(0.0)
        } else {
            0.0
        };
        let height = if viewport.1.is_finite() {
            viewport.1.max(0.0)
        } else {
            0.0
        };
        let header = f32::from(crate::tokens::size::TITLE_BAR_HEIGHT).min(height);
        let footer =
            f32::from(crate::tokens::size::STATUS_BAR_HEIGHT).min((height - header).max(0.0));
        let frame = DockRect {
            x: 0.0,
            y: header,
            width,
            height: (height - header - footer).max(0.0),
        };
        let padding = f32::from(crate::tokens::spacing::GAP).min(width / 2.0);
        let gap = f32::from(crate::tokens::spacing::GAP);
        let inner_x = padding;
        let inner_width = (width - padding * 2.0).max(0.0);
        let factor = |position| {
            if super::zone_policy::ZonePolicy::for_position(position).sizing
                == super::zone_policy::ZoneSizing::FillRemaining
            {
                1.0
            } else if detached {
                0.0
            } else {
                layout.zone(position).visual_factor(now).clamp(0.0, 1.0)
            }
        };
        let lf = factor(ZonePosition::Left);
        let rf = factor(ZonePosition::Right);
        let bf = factor(ZonePosition::Bottom);
        let horizontal_count = usize::from(lf > 0.0) + usize::from(rf > 0.0);
        let horizontal_gap = if horizontal_count == 0 {
            0.0
        } else {
            gap.min(inner_width / horizontal_count as f32)
        };
        let available_width = (inner_width - horizontal_gap * horizontal_count as f32).max(0.0);
        let mut left_ratio = layout.zone(ZonePosition::Left).extent_ratio * lf;
        let mut right_ratio = layout.zone(ZonePosition::Right).extent_ratio * rf;
        let sum = left_ratio + right_ratio;
        if sum > 1.0 {
            left_ratio /= sum;
            right_ratio /= sum;
        }
        let constrained = |position| {
            super::zone_policy::ZonePolicy::for_position(position).constrained_min_extent
        };
        let left_width = (available_width * left_ratio)
            .max(constrained(ZonePosition::Left).min(available_width) * lf);
        let right_width = (available_width * right_ratio)
            .max(constrained(ZonePosition::Right).min(available_width) * rf);
        let main_width = (available_width - left_width - right_width).max(0.0);
        let main_x = inner_x + left_width + if lf > 0.0 { horizontal_gap } else { 0.0 };
        let right_x = main_x + main_width + if rf > 0.0 { horizontal_gap } else { 0.0 };
        let vertical_gap = if bf > 0.0 { gap.min(frame.height) } else { 0.0 };
        let available_height = (frame.height - vertical_gap).max(0.0);
        let bottom_height = (available_height
            * (layout.zone(ZonePosition::Bottom).extent_ratio * bf).min(1.0))
        .max(constrained(ZonePosition::Bottom).min(available_height) * bf);
        let top_height = (available_height - bottom_height).max(0.0);
        let span_left = bf > 0.0 && lf > 0.0 && spans.0;
        let span_right = bf > 0.0 && rf > 0.0 && spans.1;
        let mut zones = BTreeMap::new();
        zones.insert(
            ZonePosition::Main,
            DockRect {
                x: main_x,
                y: 0.0,
                width: main_width,
                height: top_height,
            }
            .offset(frame.x, frame.y),
        );
        if lf > 0.0 {
            zones.insert(
                ZonePosition::Left,
                DockRect {
                    x: inner_x,
                    y: 0.0,
                    width: left_width,
                    height: if span_left { top_height } else { frame.height },
                }
                .offset(frame.x, frame.y),
            );
        }
        if rf > 0.0 {
            zones.insert(
                ZonePosition::Right,
                DockRect {
                    x: right_x,
                    y: 0.0,
                    width: right_width,
                    height: if span_right { top_height } else { frame.height },
                }
                .offset(frame.x, frame.y),
            );
        }
        if bf > 0.0 {
            let x = if span_left { inner_x } else { main_x };
            let end = if span_right {
                inner_x + inner_width
            } else {
                main_x + main_width
            };
            zones.insert(
                ZonePosition::Bottom,
                DockRect {
                    x,
                    y: top_height + vertical_gap,
                    width: (end - x).max(0.0),
                    height: bottom_height,
                }
                .offset(frame.x, frame.y),
            );
        }
        Self::from_zones(layout, frame, zones, gap)
    }

    pub fn reflow(&self, layout: &DockLayout) -> Self {
        Self::from_zones(layout, self.frame, self.zones.clone(), self.gap)
    }

    pub fn zone_resize(&self, position: ZonePosition, x: f32, y: f32) -> Option<(f32, f32)> {
        let main = self.zones.get(&ZonePosition::Main)?;
        let left = self.zones.get(&ZonePosition::Left);
        let right = self.zones.get(&ZonePosition::Right);
        let bottom = self.zones.get(&ZonePosition::Bottom);
        let available_width =
            main.width + left.map_or(0.0, |r| r.width) + right.map_or(0.0, |r| r.width);
        match position {
            ZonePosition::Left => Some((x - left?.x - self.gap / 2.0, available_width)),
            ZonePosition::Right => Some((
                right?.x + right?.width - x - self.gap / 2.0,
                available_width,
            )),
            ZonePosition::Bottom => Some((
                bottom?.y + bottom?.height - y - self.gap / 2.0,
                main.height + bottom?.height,
            )),
            ZonePosition::Main => None,
        }
    }

    #[cfg(test)]
    pub fn single_zone(layout: &DockLayout, position: ZonePosition, size: (f32, f32)) -> Self {
        let frame = DockRect {
            x: 0.0,
            y: 0.0,
            width: size.0.max(0.0),
            height: size.1.max(0.0),
        };
        Self::from_zones(
            layout,
            frame,
            BTreeMap::from([(position, frame)]),
            f32::from(crate::tokens::spacing::GAP),
        )
    }

    fn from_zones(
        layout: &DockLayout,
        frame: DockRect,
        zones: BTreeMap<ZonePosition, DockRect>,
        gap: f32,
    ) -> Self {
        let mut panels = BTreeMap::new();
        let mut panel_boundaries = Vec::new();
        for (position, rect) in &zones {
            let zone = layout.zone(*position);
            for id in layout.panel_ids().into_iter().filter(|id| {
                layout
                    .panel(id)
                    .is_some_and(|panel| panel.position == *position)
            }) {
                if let Some((x, y, width, height)) =
                    zone.visual_panel_rect(&id, (rect.width, rect.height), gap)
                {
                    panels.insert(
                        id,
                        DockRect {
                            x: rect.x + x,
                            y: rect.y + y,
                            width,
                            height,
                        },
                    );
                }
            }
            panel_boundaries.extend(
                zone.visual_boundaries((rect.width, rect.height), gap)
                    .into_iter()
                    .map(|(first, second, axis, local, available)| PanelBoundary {
                        zone: *position,
                        first,
                        second,
                        axis,
                        rect: DockRect {
                            x: rect.x + local.0,
                            y: rect.y + local.1,
                            width: local.2,
                            height: local.3,
                        },
                        available,
                    }),
            );
        }
        Self {
            frame,
            zones,
            panels,
            panel_boundaries,
            gap,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_shrink_preserves_ratios_and_does_not_mutate_layout() {
        let mut layout = DockLayout::default();
        layout.set_zone_open(ZonePosition::Right, true);
        layout.set_zone_open(ZonePosition::Bottom, true);
        let saved = layout.capture();
        let now = Instant::now() + std::time::Duration::from_secs(1);
        let geometry = |size| DockGeometry::calculate(&layout, size, false, (false, false), now);
        let large = geometry((1440.0, 900.0));
        let small = geometry((500.0, 300.0));
        let ratio = |g: &DockGeometry, p| {
            g.zones[&p].width
                / (g.zones[&ZonePosition::Left].width
                    + g.zones[&ZonePosition::Main].width
                    + g.zones[&ZonePosition::Right].width)
        };
        for p in [ZonePosition::Left, ZonePosition::Main, ZonePosition::Right] {
            assert!((ratio(&large, p) - ratio(&small, p)).abs() < 0.0001);
        }
        assert_eq!(geometry((1440.0, 900.0)).zones, large.zones);
        assert_eq!(layout.capture(), saved);
        let tiny = geometry((1.0, 1.0));
        assert!(
            tiny.zones
                .values()
                .all(|r| r.width >= 0.0 && r.height >= 0.0)
        );
    }

    #[test]
    fn detached_and_root_share_main_frame_padding() {
        let mut layout = DockLayout::default();
        layout.set_zone_open(ZonePosition::Left, false);
        let now = Instant::now() + std::time::Duration::from_secs(1);
        let root = DockGeometry::calculate(&layout, (900.0, 600.0), false, (false, false), now);
        let detached = DockGeometry::calculate(&layout, (900.0, 600.0), true, (false, false), now);
        assert_eq!(root.frame, detached.frame);
        assert_eq!(
            root.zones[&ZonePosition::Main],
            detached.zones[&ZonePosition::Main]
        );
    }

    #[test]
    fn zone_resize_uses_the_boundary_centre_without_a_size_jump() {
        let layout = DockLayout::default();
        let now = Instant::now() + std::time::Duration::from_secs(1);
        let geometry = DockGeometry::calculate(&layout, (900.0, 600.0), false, (false, false), now);
        let left = geometry.zones[&ZonePosition::Left];
        let (width, _) = geometry
            .zone_resize(
                ZonePosition::Left,
                left.x + left.width + geometry.gap / 2.0,
                left.y,
            )
            .unwrap();
        assert!((width - left.width).abs() < 0.001);
    }

    #[test]
    fn panel_limit_removes_add_target_but_keeps_existing_panel_drop() {
        let mut layout = DockLayout::default();
        let first = layout.first_panel_in_zone(ZonePosition::Left).unwrap();
        let first_extra = super::super::PanelTab::tool("first-extra");
        let second_extra = super::super::PanelTab::tool("second-extra");
        layout
            .open_tab(ZonePosition::Left, first_extra.clone())
            .unwrap();
        layout
            .open_tab(ZonePosition::Left, second_extra.clone())
            .unwrap();
        let add = |panel| PanelDrop::Edge {
            panel,
            edge: PanelEdge::Right,
        };
        let now = Instant::now();
        let second = layout
            .drop_tab(
                &first,
                &first_extra,
                &add(first.clone()),
                (900.0, 600.0),
                now,
            )
            .unwrap();
        let third = layout
            .drop_tab(&first, &second_extra, &add(second), (900.0, 600.0), now)
            .unwrap();
        let before = layout.capture();
        let geometry = DockGeometry::single_zone(&layout, ZonePosition::Left, (900.0, 600.0));
        let rect = geometry.panels[&third];
        let target = DockGeometry::drop_target(
            &layout,
            geometry.panels.iter().map(|(id, rect)| (id.clone(), *rect)),
            geometry.zones.iter().map(|(zone, rect)| (*zone, *rect)),
            (rect.x + rect.width - 1.0, rect.y + rect.height / 2.0),
        );
        assert_eq!(target, Some(PanelDrop::Into(third)));
        assert_eq!(layout.capture(), before);
    }
}
