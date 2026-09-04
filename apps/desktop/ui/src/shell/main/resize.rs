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
        if !can_resize {
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
        let (Some(target), Some(mode), Some(start)) = (
            self.shell.resizing,
            self.shell.resize_mode,
            self.shell.resize_start_pos,
        ) else {
            return;
        };

        let viewport = window.viewport_size();
        let gap = f32::from(tokens::spacing::GAP);
        let min_w = f32::from(tokens::size::PANEL_MIN_WIDTH);
        let min_h = f32::from(tokens::size::PANEL_MIN_HEIGHT);
        let shape_threshold = gap * 3.0;
        let shape_release = gap;
        let delta_x = f32::from(event.position.x - start.x);
        let delta_y = f32::from(event.position.y - start.y);
        let now = std::time::Instant::now();

        match (target, mode) {
            (ResizeTarget::Left, ResizeMode::BottomSpan) => {
                if delta_x <= -shape_threshold {
                    self.set_bottom_span_left(true, now);
                } else if delta_x >= -shape_release {
                    self.set_bottom_span_left(false, now);
                    if delta_x > 0.0 {
                        self.resize_left_width(
                            f32::from(event.position.x),
                            f32::from(viewport.width),
                            gap,
                            min_w,
                            now,
                            cx,
                        );
                    }
                }
            }
            (ResizeTarget::Right, ResizeMode::BottomSpan) => {
                if delta_x >= shape_threshold {
                    self.set_bottom_span_right(true, now);
                } else if delta_x <= shape_release {
                    self.set_bottom_span_right(false, now);
                    if delta_x < 0.0 {
                        self.resize_right_width(
                            f32::from(event.position.x),
                            f32::from(viewport.width),
                            gap,
                            min_w,
                            now,
                            cx,
                        );
                    }
                }
            }
            (ResizeTarget::Left, ResizeMode::PanelWidth) => {
                self.resize_left_width(
                    f32::from(event.position.x),
                    f32::from(viewport.width),
                    gap,
                    min_w,
                    now,
                    cx,
                );
            }
            (ResizeTarget::Right, ResizeMode::PanelWidth) => {
                self.resize_right_width(
                    f32::from(event.position.x),
                    f32::from(viewport.width),
                    gap,
                    min_w,
                    now,
                    cx,
                );
            }
            (ResizeTarget::Bottom, ResizeMode::BottomHeight) => {
                self.resize_bottom_height(
                    f32::from(event.position.y),
                    f32::from(viewport.height),
                    gap,
                    min_h,
                    cx,
                );
            }
            (ResizeTarget::Bottom, ResizeMode::RestoreLeft) => {
                if delta_y >= shape_threshold {
                    self.set_bottom_span_left(false, now);
                } else if delta_y <= shape_release {
                    self.set_bottom_span_left(true, now);
                    if delta_y < 0.0 {
                        self.resize_bottom_height(
                            f32::from(event.position.y),
                            f32::from(viewport.height),
                            gap,
                            min_h,
                            cx,
                        );
                    }
                }
            }
            (ResizeTarget::Bottom, ResizeMode::RestoreRight) => {
                if delta_y >= shape_threshold {
                    self.set_bottom_span_right(false, now);
                } else if delta_y <= shape_release {
                    self.set_bottom_span_right(true, now);
                    if delta_y < 0.0 {
                        self.resize_bottom_height(
                            f32::from(event.position.y),
                            f32::from(viewport.height),
                            gap,
                            min_h,
                            cx,
                        );
                    }
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn resize_left_width(
        &mut self,
        mouse_x: f32,
        viewport_width: f32,
        gap: f32,
        min_w: f32,
        now: std::time::Instant,
        cx: &mut Context<Self>,
    ) {
        let current_raw = mouse_x - gap;
        if current_raw < min_w * 0.5 {
            if self.shell.left_panel.open {
                self.request_panel_open_with_duration(
                    PanelPosition::Left,
                    false,
                    crate::shell::panel_transition::PANEL_SNAP_ANIMATION_DURATION,
                    cx,
                );
                self.refresh_feature_activation();
            }
            return;
        }

        let right_width = if self.shell.right_panel.is_visible(now) {
            self.shell.right_panel.effective_size(now) + gap
        } else {
            0.0
        };
        let max_left = (viewport_width - gap * 3.0 - right_width - min_w).max(min_w);
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

    fn resize_right_width(
        &mut self,
        mouse_x: f32,
        viewport_width: f32,
        gap: f32,
        min_w: f32,
        now: std::time::Instant,
        cx: &mut Context<Self>,
    ) {
        let current_raw = viewport_width - mouse_x - gap;
        if current_raw < min_w * 0.5 {
            if self.shell.right_panel.open {
                self.request_panel_open_with_duration(
                    PanelPosition::Right,
                    false,
                    crate::shell::panel_transition::PANEL_SNAP_ANIMATION_DURATION,
                    cx,
                );
                self.refresh_feature_activation();
            }
            return;
        }

        let left_width = if self.shell.left_panel.is_visible(now) {
            self.shell.left_panel.effective_size(now) + gap
        } else {
            0.0
        };
        let max_right = (viewport_width - gap * 3.0 - left_width - min_w).max(min_w);
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

    fn resize_bottom_height(
        &mut self,
        mouse_y: f32,
        viewport_height: f32,
        gap: f32,
        min_h: f32,
        cx: &mut Context<Self>,
    ) {
        let current_raw = viewport_height - mouse_y - gap;
        if current_raw < min_h * 0.5 {
            if self.shell.bottom_panel.open {
                self.request_panel_open_with_duration(
                    PanelPosition::Bottom,
                    false,
                    crate::shell::panel_transition::PANEL_SNAP_ANIMATION_DURATION,
                    cx,
                );
                self.refresh_feature_activation();
            }
            return;
        }

        let header_height = f32::from(tokens::size::TITLE_BAR_HEIGHT) + gap;
        let max_bottom = (viewport_height - header_height - min_h - gap * 2.0).max(min_h);
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

    fn set_bottom_span_left(&mut self, enabled: bool, now: std::time::Instant) {
        if self.shell.bottom_span_left == enabled {
            return;
        }
        let from_side = self.shell.left_side_shortening(now);
        let from_bottom = self.shell.bottom_left_extent(now);
        self.shell.bottom_span_left = enabled;
        self.shell.bottom_span_left_transition = Some(PanelSpanTransition::from_visual(
            from_side,
            from_bottom,
            enabled,
            now,
        ));
    }

    fn set_bottom_span_right(&mut self, enabled: bool, now: std::time::Instant) {
        if self.shell.bottom_span_right == enabled {
            return;
        }
        let from_side = self.shell.right_side_shortening(now);
        let from_bottom = self.shell.bottom_right_extent(now);
        self.shell.bottom_span_right = enabled;
        self.shell.bottom_span_right_transition = Some(PanelSpanTransition::from_visual(
            from_side,
            from_bottom,
            enabled,
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
