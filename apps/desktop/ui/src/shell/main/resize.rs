use super::*;
use crate::shell::panel_layout::Axis;
use crate::shell::state::PanelResize;

impl Shell {
    pub(super) fn start_panel_resize(
        &mut self,
        position: PanelPosition,
        path: Vec<usize>,
        pair: (usize, usize),
        axis: Axis,
        start: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let bounds = self
            .arrangement_bounds
            .borrow()
            .get(&(position, path.clone()))
            .copied();
        let Some(bounds) = bounds else {
            return;
        };
        let extent = match axis {
            Axis::Horizontal => f32::from(bounds.size.width),
            Axis::Vertical => f32::from(bounds.size.height),
        };
        let Some(available) = self.shell.layout.zone(position).available_for_resize(
            &path,
            extent,
            f32::from(tokens::spacing::GAP),
        ) else {
            return;
        };
        self.shell.panel_resize = Some(PanelResize {
            position,
            path,
            boundary: pair.0,
            next: pair.1,
            axis,
            previous: start,
            available,
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
        let bottom = self.shell.layout.zone(PanelPosition::Bottom);
        let bottom_y = f32::from(viewport.height)
            - f32::from(tokens::size::STATUS_BAR_HEIGHT)
            - bottom.size
            - gap * 2.0;
        let mode = match target {
            ResizeTarget::Left | ResizeTarget::Right
                if bottom.open && f32::from(start.y) >= bottom_y - gap * 2.0 =>
            {
                ResizeMode::BottomSpan
            }
            ResizeTarget::Left | ResizeTarget::Right => ResizeMode::PanelWidth,
            ResizeTarget::Bottom
                if self.shell.layout.bottom_span_left
                    && self.shell.layout.zone_open(PanelPosition::Left)
                    && f32::from(start.x)
                        <= self.shell.layout.zone(PanelPosition::Left).size + gap * 2.0 =>
            {
                ResizeMode::RestoreLeft
            }
            ResizeTarget::Bottom
                if self.shell.layout.bottom_span_right
                    && self.shell.layout.zone_open(PanelPosition::Right)
                    && f32::from(start.x)
                        >= f32::from(viewport.width)
                            - self.shell.layout.zone(PanelPosition::Right).size
                            - gap * 2.0 =>
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !event.dragging() {
            return;
        }
        if let Some(mut resize) = self.shell.panel_resize.take() {
            let delta = match resize.axis {
                Axis::Horizontal => f32::from(event.position.x - resize.previous.x),
                Axis::Vertical => f32::from(event.position.y - resize.previous.y),
            };
            let result = self.shell.layout.resize_panel_boundary(
                resize.position,
                &resize.path,
                resize.boundary,
                resize.next,
                delta,
                resize.available,
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
        let viewport = window.viewport_size();
        let (position, value, maximum) = match target {
            ResizeTarget::Left => (
                PanelPosition::Left,
                f32::from(event.position.x) - gap,
                f32::from(viewport.width) - gap * 3.0,
            ),
            ResizeTarget::Right => (
                PanelPosition::Right,
                f32::from(viewport.width - event.position.x) - gap,
                f32::from(viewport.width) - gap * 3.0,
            ),
            ResizeTarget::Bottom => (
                PanelPosition::Bottom,
                f32::from(viewport.height - event.position.y)
                    - f32::from(tokens::size::STATUS_BAR_HEIGHT)
                    - gap,
                f32::from(viewport.height) - f32::from(tokens::size::TITLE_BAR_HEIGHT) - gap * 3.0,
            ),
        };
        let shrinking = match target {
            ResizeTarget::Left => event.position.x < start.x,
            ResizeTarget::Right => event.position.x > start.x,
            ResizeTarget::Bottom => event.position.y > start.y,
        };
        if value < 16.0 && shrinking {
            self.shell.layout.set_zone_open(position, false);
        } else if value >= 16.0 && maximum >= 32.0 {
            self.shell.layout.zone_mut(position).size = value.clamp(32.0, maximum);
        }
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
