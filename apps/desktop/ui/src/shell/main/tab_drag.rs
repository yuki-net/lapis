use super::*;

impl Shell {
    /// Installs capture handlers that release tab drags outside the source window or cancel them on mouse down.
    /// Releases inside the source window are left to panel drop handlers.
    pub(super) fn tab_drag_events(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let shell = cx.entity().downgrade();
        gpui::canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let released = shell.clone();
                let moving = shell.clone();
                // SetCapture 中は、Window 外の MouseUp も元 Window で受け取る。
                window.on_mouse_event(move |event: &gpui::MouseUpEvent, phase, window, cx| {
                    if phase != gpui::DispatchPhase::Capture || event.button != MouseButton::Left {
                        return;
                    }
                    let active = cx.has_active_drag();
                    let _ = released.update(cx, |shell, cx| {
                        shell.close_drag_preview(cx);
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
                let moving_shell = moving.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != gpui::DispatchPhase::Capture || !cx.has_active_drag() {
                        return;
                    }
                    let Some(point) =
                        crate::window_host::drag::screen_position(event.position, window)
                    else {
                        return;
                    };
                    let _ = moving_shell.update(cx, |shell, cx| {
                        if let Some(preview) = shell.drag_preview {
                            let _ = preview.update(cx, |_, window, _| {
                                crate::window_host::drag_preview::position(window, point);
                            });
                        }
                    });
                });
                window.on_mouse_event(move |_: &MouseDownEvent, phase, _, cx| {
                    if phase == gpui::DispatchPhase::Capture {
                        let _ = shell.update(cx, |shell, cx| {
                            shell.tab_drag.cancel();
                            shell.close_drag_preview(cx);
                        });
                    }
                });
            },
        )
        .absolute()
        .size_full()
    }

    /// Clears a pending tab drag and stops GPUI's active drag when one was recorded.
    pub(crate) fn cancel_tab_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab_drag.cancel() {
            self.close_drag_preview(cx);
            cx.stop_active_drag(window);
        }
    }

    pub(crate) fn close_drag_preview(&mut self, cx: &mut App) {
        let Some(preview) = self.drag_preview.take() else {
            return;
        };
        let _ = preview.update(cx, |_, window, _| {
            crate::window_host::drag_preview::close(window);
        });
    }
}
