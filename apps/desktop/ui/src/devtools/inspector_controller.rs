use gpui::{
    AnyWindowHandle, App, Bounds, Global, InspectorElementId, Subscription, TitlebarOptions,
    Window, WindowBounds, WindowId, WindowOptions, px, size,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ClosedWindow {
    Inspector,
    Target,
    Unrelated,
}

#[derive(Default)]
struct InspectorLifecycle {
    target: Option<WindowId>,
    inspector: Option<WindowId>,
}

impl InspectorLifecycle {
    fn attach_target(&mut self, target: WindowId) -> bool {
        if self.inspector.is_some() {
            return false;
        }
        self.target = Some(target);
        true
    }

    fn attach_inspector(&mut self, inspector: WindowId) -> bool {
        if self.target.is_none() || self.inspector.is_some() {
            return false;
        }
        self.inspector = Some(inspector);
        true
    }

    fn closed(&mut self, window: WindowId) -> ClosedWindow {
        if self.inspector == Some(window) {
            self.inspector = None;
            self.target = None;
            ClosedWindow::Inspector
        } else if self.target == Some(window) {
            self.target = None;
            self.inspector = None;
            ClosedWindow::Target
        } else {
            ClosedWindow::Unrelated
        }
    }

    fn is_open(&self) -> bool {
        self.target.is_some() && self.inspector.is_some()
    }

    fn reset(&mut self) {
        self.target = None;
        self.inspector = None;
    }
}

#[derive(Default)]
pub(super) struct InspectorController {
    lifecycle: InspectorLifecycle,
    target_window: Option<AnyWindowHandle>,
    inspector_window: Option<AnyWindowHandle>,
    _window_closed_subscription: Option<Subscription>,
}

impl Global for InspectorController {}

pub(super) fn init(cx: &mut App) {
    cx.set_global(InspectorController::default());
    let subscription = cx.on_window_closed(|cx| {
        let open_windows = cx
            .windows()
            .into_iter()
            .map(|window| window.window_id())
            .collect::<Vec<_>>();
        let (closed, target_window, inspector_window) = {
            let controller = cx.global_mut::<InspectorController>();
            let closed = if controller
                .lifecycle
                .target
                .is_some_and(|window| !open_windows.contains(&window))
            {
                controller
                    .lifecycle
                    .closed(controller.lifecycle.target.unwrap())
            } else if controller
                .lifecycle
                .inspector
                .is_some_and(|window| !open_windows.contains(&window))
            {
                controller
                    .lifecycle
                    .closed(controller.lifecycle.inspector.unwrap())
            } else {
                ClosedWindow::Unrelated
            };
            let target_window = (closed != ClosedWindow::Unrelated)
                .then(|| controller.target_window.take())
                .flatten();
            let inspector_window = (closed != ClosedWindow::Unrelated)
                .then(|| controller.inspector_window.take())
                .flatten();
            (closed, target_window, inspector_window)
        };

        match closed {
            ClosedWindow::Inspector => {
                if let Some(target_window) = target_window {
                    let _ = target_window.update(cx, |_, window, _cx| window.disable_inspector());
                }
            }
            ClosedWindow::Target => {
                if let Some(inspector_window) = inspector_window {
                    let _ = inspector_window.update(cx, |_, window, _cx| window.remove_window());
                }
            }
            ClosedWindow::Unrelated => {}
        }
    });
    cx.global_mut::<InspectorController>()
        ._window_closed_subscription = Some(subscription);
}

impl InspectorController {
    pub(super) fn toggle(target_window: &mut Window, cx: &mut App) -> Result<bool, String> {
        let is_open = cx.global::<InspectorController>().lifecycle.is_open();
        if is_open {
            let inspector_window = {
                let controller = cx.global_mut::<InspectorController>();
                controller.lifecycle.reset();
                controller.target_window = None;
                controller.inspector_window.take()
            };
            target_window.disable_inspector();
            if let Some(inspector_window) = inspector_window {
                let _ = inspector_window.update(cx, |_, window, _cx| window.remove_window());
            }
            return Ok(false);
        }

        let target_handle = target_window.window_handle();
        let inspector = target_window.enable_external_inspector(cx);
        let bounds = Bounds::centered(None, size(px(900.0), px(800.0)), cx);
        let inspector_window = match cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Lapis Inspector".into()),
                    ..Default::default()
                }),
                window_min_size: Some(size(px(640.0), px(360.0))),
                ..Default::default()
            },
            move |_window, _cx| inspector,
        ) {
            Ok(window) => AnyWindowHandle::from(window),
            Err(error) => {
                target_window.disable_inspector();
                return Err(format!("Inspectorウィンドウを開けませんでした: {error}"));
            }
        };

        target_window.set_external_inspector_window(Some(inspector_window));
        let controller = cx.global_mut::<InspectorController>();
        controller.lifecycle.reset();
        let target_attached = controller
            .lifecycle
            .attach_target(target_handle.window_id());
        let inspector_attached = controller
            .lifecycle
            .attach_inspector(inspector_window.window_id());
        debug_assert!(target_attached && inspector_attached);
        controller.target_window = Some(target_handle);
        controller.inspector_window = Some(inspector_window);
        Ok(true)
    }

    pub(super) fn refresh_target(cx: &mut App) {
        let target_window = cx
            .global::<InspectorController>()
            .target_window
            .filter(|_| cx.global::<InspectorController>().lifecycle.is_open());
        if let Some(target_window) = target_window {
            let _ = target_window.update(cx, |_, window, _cx| window.refresh());
        }
    }

    pub(super) fn select_element(id: InspectorElementId, cx: &mut App) {
        let target_window = cx
            .global::<InspectorController>()
            .target_window
            .filter(|_| cx.global::<InspectorController>().lifecycle.is_open());
        if let Some(target_window) = target_window {
            let _ = target_window.update(cx, |_, window, cx| {
                window.select_inspector_element(id, cx);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_prevents_duplicate_inspector_windows_and_allows_reopen() {
        let target = WindowId::from(1);
        let inspector = WindowId::from(2);
        let replacement = WindowId::from(3);
        let mut lifecycle = InspectorLifecycle::default();

        assert!(lifecycle.attach_target(target));
        assert!(lifecycle.attach_inspector(inspector));
        assert!(lifecycle.is_open());
        assert!(!lifecycle.attach_target(target));
        assert!(!lifecycle.attach_inspector(replacement));

        assert_eq!(lifecycle.closed(inspector), ClosedWindow::Inspector);
        assert!(!lifecycle.is_open());
        assert!(lifecycle.attach_target(target));
        assert!(lifecycle.attach_inspector(replacement));
        assert!(lifecycle.is_open());
    }

    #[test]
    fn closing_target_detaches_the_inspector() {
        let target = WindowId::from(11);
        let inspector = WindowId::from(12);
        let mut lifecycle = InspectorLifecycle::default();
        assert!(lifecycle.attach_target(target));
        assert!(lifecycle.attach_inspector(inspector));

        assert_eq!(lifecycle.closed(target), ClosedWindow::Target);
        assert!(!lifecycle.is_open());
        assert_eq!(
            lifecycle.closed(WindowId::from(99)),
            ClosedWindow::Unrelated
        );
    }
}
