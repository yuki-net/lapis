use crate::components::ScrollState;

#[cfg(target_os = "windows")]
pub(super) mod native_smoke;

use gpui::{
    AnyWindowHandle, App, Bounds, EntityId, Global, InspectorElementId, Subscription,
    TitlebarOptions, Window, WindowBounds, WindowId, WindowOptions, px, size,
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
    ui: Option<InspectorUiState>,
    lifecycle: InspectorLifecycle,
    target_window: Option<AnyWindowHandle>,
    inspector_window: Option<AnyWindowHandle>,
    _window_closed_subscription: Option<Subscription>,
    inspection_subscription: Option<Subscription>,
    inspection_id: Option<EntityId>,
}

#[derive(Default)]
pub(super) struct InspectorUiState {
    pub(super) tree: super::tree_view::TreeState,
    tree_scroll: ScrollState,
    content_scroll: ScrollState,
}

impl Global for InspectorController {}

pub(super) fn init(cx: &mut App) {
    cx.set_global(InspectorController::default());
    let subscription = cx.on_window_closed(|cx| {
        if cx.global::<InspectorController>().target_window.is_none() {
            return;
        }
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
            if closed != ClosedWindow::Unrelated {
                controller.inspection_subscription = None;
                controller.inspection_id = None;
                controller.ui = None;
            }
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
                    InspectorController::release_target(target_window, cx);
                }
            }
            ClosedWindow::Target => {
                if let Some(inspector_window) = inspector_window {
                    cx.defer(move |cx| {
                        let _ = inspector_window.update(cx, |_, window, _| window.remove_window());
                    });
                }
            }
            ClosedWindow::Unrelated => {}
        }
    });
    cx.global_mut::<InspectorController>()
        ._window_closed_subscription = Some(subscription);
}

impl InspectorController {
    fn release_target(target: AnyWindowHandle, cx: &mut App) {
        cx.defer(move |cx| {
            let controller = cx.global::<Self>();
            if controller.target_window == Some(target) && controller.lifecycle.is_open() {
                return;
            }
            let _ = target.update(cx, |_, window, _| window.disable_inspector());
        });
    }

    pub(super) fn ui(cx: &mut App) -> &mut InspectorUiState {
        cx.global_mut::<Self>()
            .ui
            .get_or_insert_with(Default::default)
    }

    pub(super) fn tree_scroll(cx: &mut App) -> ScrollState {
        Self::ui(cx).tree_scroll.clone()
    }

    pub(super) fn content_scroll(cx: &mut App) -> ScrollState {
        Self::ui(cx).content_scroll.clone()
    }
    pub(super) fn toggle(target_window: &mut Window, cx: &mut App) -> Result<bool, String> {
        let is_open = cx.global::<InspectorController>().lifecycle.is_open();
        if is_open {
            let (inspector_window, previous_target) = {
                let controller = cx.global_mut::<InspectorController>();
                controller.lifecycle.reset();
                controller.inspection_subscription = None;
                controller.inspection_id = None;
                controller.ui = None;
                (
                    controller.inspector_window.take(),
                    controller.target_window.take(),
                )
            };
            if previous_target == Some(target_window.window_handle()) {
                target_window.disable_inspector();
            } else if let Some(previous_target) = previous_target {
                Self::release_target(previous_target, cx);
            }
            if let Some(inspector_window) = inspector_window {
                let _ = inspector_window.update(cx, |_, window, _cx| window.remove_window());
            }
            return Ok(false);
        }

        let target_handle = target_window.window_handle();
        // close通知の遅延中でも、新しい検査セッションへ古いIDを持ち越さない。
        target_window.disable_inspector();
        let inspector = target_window.enable_external_inspector(cx);
        let bounds = Bounds::centered(None, size(px(900.0), px(800.0)), cx);
        let observed_inspector = inspector.clone();
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

        let inspection_subscription = cx.observe(&observed_inspector, move |_, cx| {
            cx.defer(move |cx| {
                let _ = inspector_window.update(cx, |_, window, _| window.refresh());
            });
        });
        let controller = cx.global_mut::<InspectorController>();
        controller.lifecycle.reset();
        let target_attached = controller
            .lifecycle
            .attach_target(target_handle.window_id());
        let inspector_attached = controller
            .lifecycle
            .attach_inspector(inspector_window.window_id());
        debug_assert!(target_attached && inspector_attached);
        controller.inspection_id = Some(observed_inspector.entity_id());
        controller.inspection_subscription = Some(inspection_subscription);
        controller.target_window = Some(target_handle);
        controller.inspector_window = Some(inspector_window);
        Ok(true)
    }

    fn target_for(session: EntityId, cx: &App) -> Option<AnyWindowHandle> {
        let controller = cx.global::<Self>();
        controller
            .target_window
            .filter(|_| controller.lifecycle.is_open() && controller.inspection_id == Some(session))
    }

    pub(super) fn refresh_target(session: EntityId, cx: &mut App) {
        let target_window = Self::target_for(session, cx);
        if let Some(target_window) = target_window {
            let _ = target_window.update(cx, |_, window, cx| window.stop_inspector_picking(cx));
        }
    }
    pub(super) fn repaint_target(session: EntityId, cx: &mut App) {
        let target_window = Self::target_for(session, cx);
        if let Some(target_window) = target_window {
            let _ = target_window.update(cx, |_, window, _cx| window.refresh());
        }
    }

    pub(super) fn select_element(session: EntityId, id: InspectorElementId, cx: &mut App) {
        let target_window = Self::target_for(session, cx);
        if let Some(target_window) = target_window {
            let _ = target_window.update(cx, |_, window, cx| {
                window.select_inspector_element(id, cx);
            });
        }
    }
    pub(super) fn hover_element(session: EntityId, id: Option<InspectorElementId>, cx: &mut App) {
        let target_window = Self::target_for(session, cx);
        if let Some(target_window) = target_window {
            let _ = target_window.update(cx, |_, window, cx| {
                window.preview_inspector_element(id, cx);
            });
        }
    }
}

#[cfg(test)]
mod tests;
