use super::*;

impl Editor {
    pub(super) fn start_resize(
        &mut self,
        target: ResizeTarget,
        start_pos: Point<Pixels>,
        viewport: gpui::Size<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let now = std::time::Instant::now();
        let can_resize = match target {
            ResizeTarget::Left => !self.shell.left_panel.is_transitioning(),
            ResizeTarget::Right => !self.shell.right_panel.is_transitioning(),
            ResizeTarget::Bottom => !self.shell.bottom_panel.is_transitioning(),
        };
        if !can_resize || self.shell.bottom_span_is_animating(now) {
            return;
        }

        let gap = f32::from(tokens::spacing::GAP);
        let bottom_size = self.shell.bottom_panel.effective_size(now);
        let bottom_visible = self.shell.bottom_panel.is_visible(now);
        let bottom_handle_y = f32::from(viewport.height)
            - f32::from(tokens::size::STATUS_BAR_HEIGHT)
            - bottom_size
            - gap * 2.0;
        let bottom_zone_tolerance = f32::from(tokens::spacing::SM) + gap;

        let resize_mode = match target {
            ResizeTarget::Left | ResizeTarget::Right
                if bottom_visible
                    && f32::from(start_pos.y) >= bottom_handle_y - bottom_zone_tolerance =>
            {
                ResizeMode::BottomSpan
            }
            ResizeTarget::Left | ResizeTarget::Right => ResizeMode::PanelWidth,
            ResizeTarget::Bottom => {
                let start_x = f32::from(start_pos.x);
                let left_restore_edge = gap + self.shell.left_panel.effective_size(now) + gap;
                let right_restore_edge = f32::from(viewport.width)
                    - gap
                    - self.shell.right_panel.effective_size(now)
                    - gap;

                if self.shell.bottom_span_left
                    && self.shell.left_panel.is_visible(now)
                    && start_x <= left_restore_edge
                {
                    ResizeMode::RestoreLeft
                } else if self.shell.bottom_span_right
                    && self.shell.right_panel.is_visible(now)
                    && start_x >= right_restore_edge
                {
                    ResizeMode::RestoreRight
                } else {
                    ResizeMode::BottomHeight
                }
            }
        };

        self.shell.resizing = Some(target);
        self.shell.resize_start_pos = Some(start_pos);
        self.shell.resize_mode = Some(resize_mode);
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
        let (Some(target), Some(mode)) = (self.shell.resizing, self.shell.resize_mode) else {
            return;
        };

        let viewport = window.viewport_size();
        let gap = f32::from(tokens::spacing::GAP);
        let min_w = f32::from(tokens::size::PANEL_MIN_WIDTH);
        let min_h = f32::from(tokens::size::PANEL_MIN_HEIGHT);
        let collapse_threshold_w = min_w * 0.5;
        let collapse_threshold_h = min_h * 0.5;
        let now = std::time::Instant::now();

        match (target, mode) {
            (ResizeTarget::Left, ResizeMode::BottomSpan) => {
                let left_size = self.shell.left_panel.effective_size(now);
                if f32::from(event.position.x) < gap + left_size * 0.8 {
                    self.set_bottom_span_left(true, now);
                }
            }
            (ResizeTarget::Right, ResizeMode::BottomSpan) => {
                let right_size = self.shell.right_panel.effective_size(now);
                let mouse_dist_right = f32::from(viewport.width - event.position.x);
                if mouse_dist_right < gap + right_size * 0.8 {
                    self.set_bottom_span_right(true, now);
                }
            }
            (ResizeTarget::Left, ResizeMode::PanelWidth) => {
                let current_raw = f32::from(event.position.x) - gap;
                if current_raw < collapse_threshold_w {
                    if self.shell.left_panel.open {
                        self.request_panel_open_with_duration(
                            PanelPosition::Left,
                            false,
                            crate::shell::panel_transition::PANEL_SNAP_ANIMATION_DURATION,
                            cx,
                        );
                        self.refresh_feature_activation();
                    }
                } else {
                    let right_w = if self.shell.right_panel.is_visible(now) {
                        self.shell.right_panel.effective_size(now) + gap
                    } else {
                        0.0
                    };
                    let max_left =
                        (f32::from(viewport.width) - gap * 3.0 - right_w - min_w).max(min_w);
                    self.shell.left_panel.size = current_raw.clamp(min_w, max_left);
                    if !self.shell.left_panel.open {
                        self.request_panel_open_with_duration(
                            PanelPosition::Left,
                            true,
                            crate::shell::panel_transition::PANEL_SNAP_ANIMATION_DURATION,
                            cx,
                        );
                        self.refresh_feature_activation();
                    }
                }
            }
            (ResizeTarget::Right, ResizeMode::PanelWidth) => {
                let current_raw = f32::from(viewport.width - event.position.x) - gap;
                if current_raw < collapse_threshold_w {
                    if self.shell.right_panel.open {
                        self.request_panel_open_with_duration(
                            PanelPosition::Right,
                            false,
                            crate::shell::panel_transition::PANEL_SNAP_ANIMATION_DURATION,
                            cx,
                        );
                        self.refresh_feature_activation();
                    }
                } else {
                    let left_w = if self.shell.left_panel.is_visible(now) {
                        self.shell.left_panel.effective_size(now) + gap
                    } else {
                        0.0
                    };
                    let max_right =
                        (f32::from(viewport.width) - gap * 3.0 - left_w - min_w).max(min_w);
                    self.shell.right_panel.size = current_raw.clamp(min_w, max_right);
                    if !self.shell.right_panel.open {
                        self.request_panel_open_with_duration(
                            PanelPosition::Right,
                            true,
                            crate::shell::panel_transition::PANEL_SNAP_ANIMATION_DURATION,
                            cx,
                        );
                        self.refresh_feature_activation();
                    }
                }
            }
            (ResizeTarget::Bottom, ResizeMode::BottomHeight) => {
                let current_raw = f32::from(viewport.height - event.position.y) - gap;
                if current_raw < collapse_threshold_h {
                    if self.shell.bottom_panel.open {
                        self.request_panel_open_with_duration(
                            PanelPosition::Bottom,
                            false,
                            crate::shell::panel_transition::PANEL_SNAP_ANIMATION_DURATION,
                            cx,
                        );
                        self.refresh_feature_activation();
                    }
                } else {
                    let header_h = f32::from(tokens::size::TITLE_BAR_HEIGHT) + gap;
                    let max_bottom =
                        (f32::from(viewport.height) - header_h - min_h - gap * 2.0).max(min_h);
                    self.shell.bottom_panel.size = current_raw.clamp(min_h, max_bottom);
                    if !self.shell.bottom_panel.open {
                        self.request_panel_open_with_duration(
                            PanelPosition::Bottom,
                            true,
                            crate::shell::panel_transition::PANEL_SNAP_ANIMATION_DURATION,
                            cx,
                        );
                        self.refresh_feature_activation();
                    }
                }
            }
            (ResizeTarget::Bottom, ResizeMode::RestoreLeft)
                if self.dragged_down_by(event.position, gap * 3.0) =>
            {
                self.set_bottom_span_left(false, now);
            }
            (ResizeTarget::Bottom, ResizeMode::RestoreRight)
                if self.dragged_down_by(event.position, gap * 3.0) =>
            {
                self.set_bottom_span_right(false, now);
            }
            _ => {}
        }
        cx.notify();
    }

    fn dragged_down_by(&self, position: Point<Pixels>, threshold: f32) -> bool {
        self.shell
            .resize_start_pos
            .is_some_and(|start| f32::from(position.y - start.y) >= threshold)
    }

    fn set_bottom_span_left(&mut self, enabled: bool, now: std::time::Instant) {
        if self.shell.bottom_span_left == enabled
            || self
                .shell
                .bottom_span_left_transition
                .as_ref()
                .is_some_and(|transition| transition.is_active(now))
        {
            return;
        }
        let from = self.shell.bottom_span_left_value(now);
        self.shell.bottom_span_left = enabled;
        self.shell.bottom_span_left_transition = Some(PanelSpanTransition::new(
            from,
            if enabled { 1.0 } else { 0.0 },
            now,
        ));
    }

    fn set_bottom_span_right(&mut self, enabled: bool, now: std::time::Instant) {
        if self.shell.bottom_span_right == enabled
            || self
                .shell
                .bottom_span_right_transition
                .as_ref()
                .is_some_and(|transition| transition.is_active(now))
        {
            return;
        }
        let from = self.shell.bottom_span_right_value(now);
        self.shell.bottom_span_right = enabled;
        self.shell.bottom_span_right_transition = Some(PanelSpanTransition::new(
            from,
            if enabled { 1.0 } else { 0.0 },
            now,
        ));
    }

    pub(super) fn stop_resize(&mut self, cx: &mut Context<Self>) {
        if self.shell.resizing.take().is_some() {
            self.shell.resize_start_pos = None;
            self.shell.resize_mode = None;
            cx.notify();
        }
    }
}
