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
        match target {
            ResizeTarget::Left => {
                self.shell.left_panel.size = (f32::from(event.position.x)
                    - f32::from(tokens::spacing::GAP))
                .clamp(190.0, 380.0);
            }
            ResizeTarget::Right => {
                self.shell.right_panel.size = (f32::from(viewport.width - event.position.x)
                    - f32::from(tokens::spacing::GAP))
                .clamp(260.0, 480.0);
            }
            ResizeTarget::Bottom => {
                self.shell.bottom_panel.size = (f32::from(viewport.height - event.position.y)
                    - f32::from(tokens::spacing::GAP))
                .clamp(140.0, 360.0);
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
