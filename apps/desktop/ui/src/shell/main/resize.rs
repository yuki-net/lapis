use super::*;

impl Editor {
    pub(super) fn start_resize(&mut self, target: ResizeTarget, cx: &mut Context<Self>) {
        let can_resize = match target {
            ResizeTarget::Left => !self.shell.left_panel.is_transitioning(),
            ResizeTarget::Right => !self.shell.right_panel.is_transitioning(),
            ResizeTarget::Bottom => !self.shell.bottom_panel.is_transitioning(),
        };
        if !can_resize {
            return;
        }
        self.shell.resizing = Some(target);
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
        let Some(target) = self.shell.resizing else {
            return;
        };

        let viewport = window.viewport_size();
        let gap = f32::from(tokens::spacing::GAP);
        let min_w = f32::from(tokens::size::PANEL_MIN_WIDTH);
        let min_h = f32::from(tokens::size::PANEL_MIN_HEIGHT);
        let collapse_threshold_w = min_w * 0.5;
        let collapse_threshold_h = min_h * 0.5;
        let now = std::time::Instant::now();

        match target {
            ResizeTarget::Left => {
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
                        (f32::from(viewport.width) - gap * 2.0 - right_w - min_w).max(min_w);
                    let clamped = current_raw.clamp(min_w, max_left);
                    self.shell.left_panel.size = clamped;
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
            ResizeTarget::Right => {
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
                        (f32::from(viewport.width) - gap * 2.0 - left_w - min_w).max(min_w);
                    let clamped = current_raw.clamp(min_w, max_right);
                    self.shell.right_panel.size = clamped;
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
            ResizeTarget::Bottom => {
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
                    let clamped = current_raw.clamp(min_h, max_bottom);
                    self.shell.bottom_panel.size = clamped;
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
        }
        cx.notify();
    }

    pub(super) fn stop_resize(&mut self, cx: &mut Context<Self>) {
        if self.shell.resizing.take().is_some() {
            cx.notify();
        }
    }
}
