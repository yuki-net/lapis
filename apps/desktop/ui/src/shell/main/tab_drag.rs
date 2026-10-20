use super::*;

impl Shell {
    pub(super) fn tab_drag_events(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let shell = cx.entity().downgrade();
        gpui::canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let released = shell.clone();
                // SetCapture 中は、Window 外の MouseUp も元 Window で受け取る。
                window.on_mouse_event(move |event: &gpui::MouseUpEvent, phase, window, cx| {
                    if phase != gpui::DispatchPhase::Capture {
                        return;
                    }
                    let active = cx.has_active_drag() && event.button == MouseButton::Left;
                    let _ = released.update(cx, |shell, cx| {
                        let Some(drag) = shell.tab_drag.release(active) else {
                            return;
                        };
                        let point =
                            crate::window_host::drag::screen_position(event.position, window);
                        let inside = gpui::Bounds::new(Point::default(), window.viewport_size())
                            .contains(&event.position);
                        if inside
                            && point.is_none_or(|point| {
                                crate::window_host::drag::is_source_window(window, point)
                            })
                        {
                            // 同じ Window 内は既存の Panel on_drop に任せる。
                            return;
                        }
                        if let Some(point) = point {
                            cx.emit(UiCommand::ReleaseTab(drag, point));
                        }
                        cx.stop_active_drag(window);
                        cx.stop_propagation();
                    });
                });
                window.on_mouse_event(move |_: &MouseDownEvent, phase, _, cx| {
                    if phase == gpui::DispatchPhase::Capture {
                        let _ = shell.update(cx, |shell, _| shell.tab_drag.cancel());
                    }
                });
            },
        )
        .absolute()
        .size_full()
    }

    pub(crate) fn cancel_tab_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab_drag.cancel() {
            cx.stop_active_drag(window);
        }
    }
}
