use super::*;
use gpui::{
    AppContext, Context, DivInspectorState, Entity, Inspector, IntoElement, Modifiers, Render,
    TestAppContext, VisualTestContext, div, prelude::*,
};
use std::{cell::Cell, rc::Rc};

struct CachedChild;

impl Render for CachedChild {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("cached-child")
            .debug_selector(|| "cached-child".into())
            .w(px(80.0))
            .h(px(40.0))
    }
}

struct Target {
    cached: Entity<CachedChild>,
    show_sample: bool,
}

impl Target {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            cached: cx.new(|_| CachedChild),
            show_sample: true,
        }
    }
}

impl Render for Target {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("target")
            .debug_selector(|| "target".into())
            .size_full()
            .p_4()
            .when(self.show_sample, |element| {
                element.child(
                    div()
                        .id("sample")
                        .debug_selector(|| "sample".into())
                        .w(px(180.0))
                        .h(px(100.0))
                        .p_2()
                        .border_1()
                        .child(gpui::AnyView::from(self.cached.clone()).cached(Default::default())),
                )
            })
    }
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.draw(cx)).clear();
    cx.run_until_parked();
}

fn node(
    inspector: &Entity<Inspector>,
    name: &str,
    cx: &mut VisualTestContext,
) -> InspectorElementId {
    inspector.read_with(cx, |inspector, _| {
        inspector
            .element_tree()
            .iter()
            .find(|node| node.id.path.global_id.to_string().ends_with(name))
            .unwrap()
            .id
            .clone()
    })
}

#[gpui::test]
fn external_tree_selection_hover_and_cached_children_keep_full_width(cx: &mut TestAppContext) {
    let (target, cx) = cx.add_window_view(|_, cx| Target::new(cx));
    cx.simulate_resize(size(px(1200.0), px(800.0)));
    draw(cx);
    let width = cx.debug_bounds("target").unwrap().size.width;
    let inspector = cx.update(|window, cx| {
        crate::devtools::init(cx);
        window.enable_external_inspector(cx)
    });
    draw(cx);
    assert_eq!(cx.debug_bounds("target").unwrap().size.width, width);
    let sample = node(&inspector, "sample", cx);
    let cached = node(&inspector, "cached-child", cx);
    inspector.read_with(cx, |inspector, _| {
        let child = inspector
            .element_tree()
            .iter()
            .find(|node| node.id == cached)
            .unwrap();
        assert_eq!(child.parent.as_ref(), Some(&sample));
        assert_eq!(child.bounds.size.width, px(80.0));
        assert!(!child.id.path.source_location.file().is_empty());
    });
    assert!(cx.update(|window, cx| window.select_inspector_element(sample.clone(), cx)));
    draw(cx);
    inspector.read_with(cx, |inspector, _| {
        assert_eq!(inspector.active_element_id(), Some(&sample));
        let state = inspector
            .active_element_state::<DivInspectorState>()
            .unwrap();
        assert_eq!(state.bounds.size.width, px(180.0));
        assert!(state.base_style.padding.top.is_some());
        assert!(state.base_style.border_widths.top.is_some());
    });
    assert!(cx.update(|window, cx| window.preview_inspector_element(Some(cached.clone()), cx)));
    draw(cx);
    inspector.read_with(cx, |inspector, _| {
        assert_eq!(inspector.hovered_element_id(), Some(&cached));
        assert_eq!(inspector.active_element_id(), Some(&sample));
        assert!(
            inspector
                .hovered_element_state::<DivInspectorState>()
                .is_some()
        );
    });
    let revision = inspector.read_with(cx, |inspector, _| inspector.tree_revision());
    draw(cx);
    assert_eq!(
        inspector.read_with(cx, |inspector, _| inspector.tree_revision()),
        revision
    );
    assert_eq!(node(&inspector, "cached-child", cx), cached);
    let mut tree = crate::devtools::tree_view::TreeState::default();
    let full_rows = inspector.read_with(cx, |inspector, _| tree.rows(inspector).len());
    tree.toggle(sample.clone());
    assert!(inspector.read_with(cx, |inspector, _| tree.rows(inspector).len()) < full_rows);
    cx.update(|window, cx| {
        window.select_inspector_element(cached.clone(), cx);
    });
    draw(cx);
    assert_eq!(
        inspector.read_with(cx, |inspector, _| tree.rows(inspector).len()),
        full_rows
    );
    target.update(cx, |target, cx| {
        target.show_sample = false;
        cx.notify();
    });
    draw(cx);
    inspector.read_with(cx, |inspector, _| {
        assert!(inspector.active_element_id().is_none());
        assert!(inspector.hovered_element_id().is_none());
    });
    assert!(!cx.update(|window, cx| window.select_inspector_element(sample, cx)));
}

#[gpui::test]
fn pick_depth_and_tree_state_are_independent_per_target(cx: &mut TestAppContext) {
    let (_, first) = cx.add_window_view(|_, cx| Target::new(cx));
    let one = first.update(|window, cx| window.enable_external_inspector(cx));
    draw(first);
    let sample = node(&one, "sample", first);
    let first_handle = first.update(|window, _| window.window_handle());
    let (_, second) = first.add_window_view(|_, cx| Target::new(cx));
    let two = second.update(|window, cx| window.enable_external_inspector(cx));
    draw(second);
    let other = node(&two, "sample", second);
    assert!(second.update(|window, cx| window.select_inspector_element(other.clone(), cx)));
    first_handle
        .update(second, |_, window, cx| {
            one.update(cx, |inspector, _| inspector.start_picking());
            window.refresh();
        })
        .unwrap();
    let mut first = VisualTestContext::from_window(first_handle, second);
    draw(&mut first);
    let point = first.debug_bounds("sample").unwrap().origin + gpui::point(px(4.0), px(4.0));
    first.simulate_mouse_move(point, None, Modifiers::default());
    draw(&mut first);
    first.simulate_event(gpui::ScrollWheelEvent {
        position: point,
        delta: gpui::ScrollDelta::Lines(gpui::point(0.0, 3.0)),
        modifiers: Modifiers::default(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    draw(&mut first);
    one.read_with(&mut first, |inspector, _| assert!(inspector.is_picking()));
    first.simulate_click(point, Modifiers::default());
    draw(&mut first);
    one.read_with(&mut first, |inspector, _| {
        assert!(!inspector.is_picking());
        assert!(inspector.active_element_id().is_some());
    });
    two.read_with(&mut first, |inspector, _| {
        assert_eq!(inspector.active_element_id(), Some(&other));
        assert!(!inspector.is_picking());
        assert!(inspector.hovered_element_id().is_none());
    });
    first.update(|window, cx| {
        window.select_inspector_element(sample, cx);
    });
    draw(&mut first);
    let mut tree = crate::devtools::tree_view::TreeState::default();
    let (rows, again) = one.read_with(&mut first, |inspector, _| {
        (tree.rows(inspector), tree.rows(inspector))
    });
    assert!(Rc::ptr_eq(&rows, &again));
}

#[gpui::test]
fn controller_closes_reopens_and_rejects_late_window_updates(cx: &mut TestAppContext) {
    let (_, target) = cx.add_window_view(|_, cx| Target::new(cx));
    target.update(|window, cx| {
        crate::devtools::init(cx);
        assert_eq!(InspectorController::toggle(window, cx), Ok(true));
    });
    draw(target);
    let viewer =
        target.update(|_, cx| cx.global::<InspectorController>().inspector_window.unwrap());
    let inspector = viewer
        .downcast::<Inspector>()
        .unwrap()
        .root(target)
        .unwrap();
    let weak = inspector.downgrade();
    let old_session = inspector.entity_id();
    let mut viewer_cx = VisualTestContext::from_window(viewer, target);
    draw(&mut viewer_cx);
    let stale = node(&inspector, "sample", target);
    let index = inspector.read_with(target, |inspector, _| {
        inspector
            .element_tree()
            .iter()
            .position(|node| node.id == stale)
            .unwrap()
    });
    let row = viewer_cx
        .debug_bounds(&format!("inspector-tree-row-{index}"))
        .unwrap();
    viewer_cx.simulate_mouse_move(row.center(), None, Modifiers::default());
    draw(target);
    inspector.read_with(target, |inspector, _| {
        assert_eq!(inspector.hovered_element_id(), Some(&stale))
    });
    viewer_cx.simulate_click(row.center(), Modifiers::default());
    draw(target);
    draw(&mut viewer_cx);
    inspector.read_with(target, |inspector, _| {
        assert_eq!(inspector.active_element_id(), Some(&stale))
    });
    // 購読通知がdeferした更新より先にviewerを破棄する。
    target.update(|_, cx| {
        inspector.update(cx, |_, cx| cx.notify());
        viewer
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
    });
    target.run_until_parked();
    assert!(!target.update(|_, cx| cx.global::<InspectorController>().lifecycle.is_open()));
    assert!(viewer.update(target, |_, _, _| ()).is_err());
    drop(inspector);
    assert!(weak.upgrade().is_none());
    assert!(!target.update(|window, cx| window.select_inspector_element(stale.clone(), cx)));
    target.update(|window, cx| assert_eq!(InspectorController::toggle(window, cx), Ok(true)));
    draw(target);
    let replacement =
        target.update(|_, cx| cx.global::<InspectorController>().inspector_window.unwrap());
    assert!(replacement != viewer);
    target.update(|_, cx| {
        InspectorController::select_element(old_session, stale.clone(), cx);
        InspectorController::hover_element(old_session, Some(stale), cx);
        InspectorController::refresh_target(old_session, cx);
    });
    let current = replacement
        .downcast::<Inspector>()
        .unwrap()
        .root(target)
        .unwrap();
    current.read_with(target, |inspector, _| {
        assert!(inspector.active_element_id().is_none());
        assert!(inspector.hovered_element_id().is_none());
    });
    target.update(|window, _| window.remove_window());
    target.run_until_parked();
    assert!(replacement.update(target, |_, _, _| ()).is_err());
}

struct Probe {
    element: gpui::Div,
    calls: Rc<Cell<usize>>,
}

impl gpui::IntoElement for Probe {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl gpui::Element for Probe {
    type RequestLayoutState = <gpui::Div as gpui::Element>::RequestLayoutState;
    type PrepaintState = <gpui::Div as gpui::Element>::PrepaintState;
    fn id(&self) -> Option<gpui::ElementId> {
        Some("probe".into())
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        self.calls.set(self.calls.get() + 1);
        Some(std::panic::Location::caller())
    }
    fn request_layout(
        &mut self,
        id: Option<&gpui::GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, Self::RequestLayoutState) {
        self.element.request_layout(id, inspector, window, cx)
    }
    fn prepaint(
        &mut self,
        id: Option<&gpui::GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<gpui::Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.element
            .prepaint(id, inspector, bounds, state, window, cx)
    }
    fn paint(
        &mut self,
        id: Option<&gpui::GlobalElementId>,
        inspector: Option<&InspectorElementId>,
        bounds: Bounds<gpui::Pixels>,
        state: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.element
            .paint(id, inspector, bounds, state, prepaint, window, cx);
    }
}

struct ProbeTarget(Rc<Cell<usize>>);
impl Render for ProbeTarget {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Probe {
            element: div().size_full(),
            calls: self.0.clone(),
        }
    }
}

#[gpui::test]
fn inspection_does_not_collect_when_disabled_and_releases_its_entity(cx: &mut TestAppContext) {
    let calls = Rc::new(Cell::new(0));
    let (_, cx) = cx.add_window_view(|_, _| ProbeTarget(calls.clone()));
    draw(cx);
    assert_eq!(calls.get(), 0);
    let inspector = cx.update(|window, cx| window.enable_external_inspector(cx));
    draw(cx);
    assert!(calls.get() > 0);
    assert!(!inspector.read_with(cx, |inspector, _| inspector.element_tree().is_empty()));
    let weak = inspector.downgrade();
    drop(inspector);
    cx.update(|window, _| window.disable_inspector());
    calls.set(0);
    draw(cx);
    assert_eq!(calls.get(), 0);
    assert!(weak.upgrade().is_none());
}

#[gpui::test]
fn toggling_from_another_window_releases_the_original_target(cx: &mut TestAppContext) {
    let (_, first) = cx.add_window_view(|_, cx| Target::new(cx));
    first.update(|window, cx| {
        crate::devtools::init(cx);
        assert_eq!(InspectorController::toggle(window, cx), Ok(true));
    });
    draw(first);
    let original = first.update(|window, _| window.window_handle());
    let viewer = first.update(|_, cx| cx.global::<InspectorController>().inspector_window.unwrap());
    let inspector = viewer.downcast::<Inspector>().unwrap().root(first).unwrap();
    let id = node(&inspector, "sample", first);
    let weak = inspector.downgrade();
    drop(inspector);
    let (_, second) = first.add_window_view(|_, cx| Target::new(cx));
    second.update(|window, cx| assert_eq!(InspectorController::toggle(window, cx), Ok(false)));
    second.run_until_parked();
    assert!(weak.upgrade().is_none());
    assert!(viewer.update(second, |_, _, _| ()).is_err());
    assert!(
        !original
            .update(second, |_, window, cx| window
                .select_inspector_element(id, cx))
            .unwrap()
    );
    assert!(second.update(|_, cx| cx.global::<InspectorController>().ui.is_none()));
}

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
