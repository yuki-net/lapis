use super::*;
use crate::shell::panel_layout::Axis;
use crate::shell::state::PanelResize;

impl Shell {
    pub(super) fn start_panel_resize(
        &mut self,
        boundary: crate::shell::dock_geometry::PanelBoundary,
        start: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.shell.panel_resize = Some(PanelResize {
            boundary,
            previous: start,
        });
        cx.notify();
    }

    pub(super) fn start_resize(
        &mut self,
        target: ResizeTarget,
        start: Point<Pixels>,
        viewport: gpui::Size<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let gap = f32::from(tokens::spacing::GAP);
        let bottom = self.shell.layout.zone(ZonePosition::Bottom);
        let geometry = self.dock_geometry.borrow();
        let bottom_y = geometry
            .as_ref()
            .and_then(|geometry| geometry.zones.get(&ZonePosition::Bottom))
            .map_or(f32::from(viewport.height), |rect| rect.y);
        let left_width = geometry
            .as_ref()
            .and_then(|geometry| geometry.zones.get(&ZonePosition::Left))
            .map_or(0.0, |rect| rect.width);
        let right_width = geometry
            .as_ref()
            .and_then(|geometry| geometry.zones.get(&ZonePosition::Right))
            .map_or(0.0, |rect| rect.width);
        let mode = match target {
            ResizeTarget::Left | ResizeTarget::Right
                if bottom.open && f32::from(start.y) >= bottom_y - gap * 2.0 =>
            {
                ResizeMode::BottomSpan
            }
            ResizeTarget::Left | ResizeTarget::Right => ResizeMode::PanelWidth,
            ResizeTarget::Bottom
                if self.shell.layout.bottom_span_left
                    && self.shell.layout.zone_open(ZonePosition::Left)
                    && f32::from(start.x) <= left_width + gap * 2.0 =>
            {
                ResizeMode::RestoreLeft
            }
            ResizeTarget::Bottom
                if self.shell.layout.bottom_span_right
                    && self.shell.layout.zone_open(ZonePosition::Right)
                    && f32::from(start.x)
                        >= f32::from(viewport.width) - right_width - gap * 2.0 =>
            {
                ResizeMode::RestoreRight
            }
            ResizeTarget::Bottom => ResizeMode::BottomHeight,
        };
        self.shell.resizing = Some(target);
        self.shell.resize_start_pos = Some(start);
        self.shell.resize_mode = Some(mode);
        cx.notify();
    }

    pub(super) fn resize_panels(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !event.dragging() {
            return;
        }
        if let Some(mut resize) = self.shell.panel_resize.take() {
            let delta = match resize.boundary.axis {
                Axis::Horizontal => f32::from(event.position.x - resize.previous.x),
                Axis::Vertical => f32::from(event.position.y - resize.previous.y),
            };
            let result = self.shell.layout.resize_between_panels(
                resize.boundary.zone,
                &resize.boundary.first,
                &resize.boundary.second,
                delta,
                resize.boundary.available,
            );
            if result.as_ref().is_ok_and(|closed| closed.is_none()) {
                resize.previous = event.position;
                self.shell.panel_resize = Some(resize);
            }
            cx.notify();
            return;
        }
        let (Some(target), Some(mode), Some(start)) = (
            self.shell.resizing,
            self.shell.resize_mode,
            self.shell.resize_start_pos,
        ) else {
            return;
        };
        let gap = f32::from(tokens::spacing::GAP);
        if mode == ResizeMode::BottomSpan {
            let delta = f32::from(event.position.x - start.x);
            let now = std::time::Instant::now();
            match target {
                ResizeTarget::Left => self.set_bottom_span_left(delta <= -gap * 3.0, now),
                ResizeTarget::Right => self.set_bottom_span_right(delta >= gap * 3.0, now),
                _ => {}
            }
            cx.notify();
            return;
        }
        if matches!(mode, ResizeMode::RestoreLeft | ResizeMode::RestoreRight) {
            let delta = f32::from(event.position.y - start.y);
            let now = std::time::Instant::now();
            match mode {
                ResizeMode::RestoreLeft if delta >= gap * 3.0 => {
                    self.set_bottom_span_left(false, now)
                }
                ResizeMode::RestoreRight if delta >= gap * 3.0 => {
                    self.set_bottom_span_right(false, now)
                }
                ResizeMode::RestoreLeft if delta <= gap => self.set_bottom_span_left(true, now),
                ResizeMode::RestoreRight if delta <= gap => self.set_bottom_span_right(true, now),
                _ => {}
            }
            if delta >= 0.0 {
                cx.notify();
                return;
            }
        }
        let position = match target {
            ResizeTarget::Left => ZonePosition::Left,
            ResizeTarget::Right => ZonePosition::Right,
            ResizeTarget::Bottom => ZonePosition::Bottom,
        };
        let Some((value, maximum)) = self.dock_geometry.borrow().as_ref().and_then(|geometry| {
            geometry.zone_resize(
                position,
                f32::from(event.position.x),
                f32::from(event.position.y),
            )
        }) else {
            return;
        };
        let shrinking = match target {
            ResizeTarget::Left => event.position.x < start.x,
            ResizeTarget::Right => event.position.x > start.x,
            ResizeTarget::Bottom => event.position.y > start.y,
        };
        self.shell
            .layout
            .resize_zone(position, value, maximum, shrinking);
        cx.notify();
    }

    fn set_bottom_span_left(&mut self, enabled: bool, now: std::time::Instant) {
        if self.shell.layout.bottom_span_left == enabled {
            return;
        }
        let from_side = self.shell.left_side_shortening(now);
        let from_bottom = self.shell.bottom_left_extent(now);
        self.shell.layout.bottom_span_left = enabled;
        self.shell.bottom_span_left_transition = Some(PanelSpanTransition::from_visual(
            from_side,
            from_bottom,
            enabled,
            now,
        ));
    }
    fn set_bottom_span_right(&mut self, enabled: bool, now: std::time::Instant) {
        if self.shell.layout.bottom_span_right == enabled {
            return;
        }
        let from_side = self.shell.right_side_shortening(now);
        let from_bottom = self.shell.bottom_right_extent(now);
        self.shell.layout.bottom_span_right = enabled;
        self.shell.bottom_span_right_transition = Some(PanelSpanTransition::from_visual(
            from_side,
            from_bottom,
            enabled,
            now,
        ));
    }
    pub(super) fn stop_resize(&mut self, cx: &mut Context<Self>) {
        if self.shell.resizing.take().is_some() || self.shell.panel_resize.take().is_some() {
            self.shell.resize_start_pos = None;
            self.shell.resize_mode = None;
            cx.notify();
        }
    }
}
