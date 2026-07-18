use super::*;

impl Editor {
    pub(super) fn start_resize(&mut self, target: ResizeTarget, cx: &mut Context<Self>) {
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
            ResizeTarget::ToolIsland => {
                self.shell.tool_island_width =
                    (f32::from(event.position.x) - theme::CANVAS_GAP).clamp(190.0, 380.0);
            }
            ResizeTarget::SidePanel => {
                self.shell.side_panel_width = (f32::from(viewport.width - event.position.x)
                    - theme::CANVAS_GAP)
                    .clamp(260.0, 480.0);
            }
            ResizeTarget::BottomPanel => {
                self.shell.bottom_panel_height = (f32::from(viewport.height - event.position.y)
                    - theme::CANVAS_GAP)
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
