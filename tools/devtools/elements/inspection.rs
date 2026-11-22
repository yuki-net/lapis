//! GPUI検査APIとの境界。Windowの表示・Treeの開閉状態はLapisが所有する。
use gpui::{AnyWindowHandle, App, Entity, Inspector, InspectorElementId, Subscription, Window};

pub(crate) struct Inspection {
    inspector: Entity<Inspector>,
    _updates: Subscription,
}

impl Inspection {
    pub(crate) fn attach(
        inspector: Entity<Inspector>,
        viewer: AnyWindowHandle,
        cx: &mut App,
    ) -> Self {
        // GPUIのFrame完了通知を購読する。破棄済みviewerへの遅延通知はhandle検証で捨てる。
        let updates = cx.subscribe(&inspector, move |_, _: &gpui::InspectorUpdated, cx| {
            cx.defer(move |cx| {
                let _ = viewer.update(cx, |_, window, _| window.refresh());
            });
        });
        Self {
            inspector,
            _updates: updates,
        }
    }

    pub(crate) fn enable(window: &mut Window, cx: &mut App) -> Entity<Inspector> {
        let inspector = window.enable_external_inspector(cx);
        inspector.update(cx, |inspector, _| {
            inspector.set_overlay_renderer(super::overlay::paint_overlay)
        });
        inspector
    }

    pub(crate) fn disable(window: &mut Window) {
        window.disable_inspector();
    }

    pub(crate) fn entity(&self) -> Entity<Inspector> {
        self.inspector.clone()
    }

    pub(crate) fn refresh(inspector: &Entity<Inspector>, window: &mut Window, cx: &mut App) {
        inspector.update(cx, |inspector, cx| {
            inspector.stop_picking();
            cx.notify();
        });
        window.refresh();
    }

    pub(crate) fn select(id: InspectorElementId, window: &mut Window, cx: &mut App) {
        window.select_inspector_element(id, cx);
    }

    pub(crate) fn hover(id: Option<InspectorElementId>, window: &mut Window, cx: &mut App) {
        window.preview_inspector_element(id, cx);
    }
}
