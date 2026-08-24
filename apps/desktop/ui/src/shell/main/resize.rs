use super::*;

impl Editor {
    pub(super) fn start_resize(&mut self, target: ResizeTarget, cx: &mut Context<Self>) {
        let can_resize = match target {
            ResizeTarget::Left => {
                self.shell.left_panel.open && !self.shell.left_panel.is_transitioning()
            }
            ResizeTarget::Right => {
                self.shell.right_panel.open && !self.shell.right_panel.is_transitioning()
            }
            ResizeTarget::Bottom => {
                self.shell.bottom_panel.open && !self.shell.bottom_panel.is_transitioning()
            }
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
        let now = std::time::Instant::now();

        match target {
            ResizeTarget::Left => {
                let right_w = if self.shell.right_panel.is_visible(now) {
                    self.shell.right_panel.effective_size(now) + gap
                } else {
                    0.0
                };
                let max_left = (f32::from(viewport.width) - gap * 2.0 - right_w - min_w).max(min_w);
                self.shell.left_panel.size =
                    (f32::from(event.position.x) - gap).clamp(min_w, max_left);
            }
            ResizeTarget::Right => {
                let left_w = if self.shell.left_panel.is_visible(now) {
                    self.shell.left_panel.effective_size(now) + gap
                } else {
                    0.0
                };
                let max_right = (f32::from(viewport.width) - gap * 2.0 - left_w - min_w).max(min_w);
                self.shell.right_panel.size =
                    (f32::from(viewport.width - event.position.x) - gap).clamp(min_w, max_right);
            }
            ResizeTarget::Bottom => {
                let header_h = f32::from(tokens::size::TITLE_BAR_HEIGHT) + gap;
                let max_bottom =
                    (f32::from(viewport.height) - header_h - min_h - gap * 2.0).max(min_h);
                self.shell.bottom_panel.size =
                    (f32::from(viewport.height - event.position.y) - gap).clamp(min_h, max_bottom);
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
