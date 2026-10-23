use super::{inspection::Inspection, inspector_controller::InspectorController};
use gpui::{
    Context, DivInspectorState, Entity, Inspector, IntoElement, Modifiers, Pixels, Render,
    ScrollDelta, ScrollWheelEvent, TestAppContext, TouchPhase, VisualTestContext, Window,
    WindowHandle, div, point, prelude::*, px, rgb,
};

struct Target {
    width: Pixels,
    child: bool,
}
impl Render for Target {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().id("fixture-root").size_full().child(
            div()
                .id("fixture-parent")
                .w(self.width)
                .h(px(150.))
                .p_4()
                .border_2()
                .when(self.child, |el| {
                    el.child(
                        div()
                            .id("fixture-child")
                            .w(px(60.))
                            .h(px(40.))
                            .bg(rgb(0xff0000)),
                    )
                }),
        )
    }
}

fn setup(cx: &mut TestAppContext) -> WindowHandle<Target> {
    cx.update(super::init);
    cx.add_window(|_, _| Target {
        width: px(200.),
        child: true,
    })
}

fn draw(target: WindowHandle<Target>, cx: &mut TestAppContext) {
    cx.update_window(target.into(), |_, window, cx| {
        window.refresh();
        window.draw(cx).clear();
    })
    .unwrap();
    cx.run_until_parked();
}

fn inspector(cx: &mut TestAppContext) -> Entity<Inspector> {
    let viewer = cx.read(|cx| cx.global::<InspectorController>().inspector_handle());
    viewer
        .unwrap()
        .downcast::<Inspector>()
        .unwrap()
        .root(cx)
        .unwrap()
}

#[gpui::test]
fn external_tree_selection_hover_details_and_refresh(cx: &mut TestAppContext) {
    let target = setup(cx);
    let before = target
        .update(cx, |_, window, _| window.viewport_size())
        .unwrap();
    target
        .update(cx, |_, window, cx| {
            InspectorController::toggle(window, cx).unwrap()
        })
        .unwrap();
    draw(target, cx);
    let inspector = inspector(cx);
    let (parent, child) = inspector.read_with(cx, |inspector, _| {
        let nodes = inspector.element_tree();
        let parent = nodes
            .iter()
            .find(|node| {
                node.id
                    .path
                    .global_id
                    .to_string()
                    .ends_with("fixture-parent")
            })
            .unwrap();
        let child = nodes
            .iter()
            .find(|node| {
                node.id
                    .path
                    .global_id
                    .to_string()
                    .ends_with("fixture-child")
            })
            .unwrap();
        assert_eq!(child.parent.as_ref(), Some(&parent.id));
        assert_eq!(parent.bounds.size.width, px(200.));
        (parent.id.clone(), child.id.clone())
    });
    target
        .update(cx, |_, window, cx| {
            assert_eq!(window.viewport_size(), before);
            assert!(window.select_inspector_element(child.clone(), cx));
            assert!(window.preview_inspector_element(Some(parent.clone()), cx));
        })
        .unwrap();
    draw(target, cx);
    inspector.read_with(cx, |inspector, _| {
        assert_eq!(inspector.active_element_id(), Some(&child));
        assert_eq!(inspector.hovered_element_id(), Some(&parent));
        let details = inspector
            .active_element_state::<DivInspectorState>()
            .unwrap();
        assert_eq!(details.bounds.size.width, px(60.));
        assert_eq!(
            details.base_style.background.as_ref(),
            Some(&rgb(0xff0000).into())
        );
        assert_eq!(
            inspector
                .hovered_element_state::<DivInspectorState>()
                .unwrap()
                .bounds
                .size
                .width,
            px(200.)
        );
    });
    target
        .update(cx, |target, _, _| {
            target.width = px(250.);
            target.child = false;
        })
        .unwrap();
    draw(target, cx);
    inspector.read_with(cx, |inspector, _| {
        assert!(inspector.active_element_id().is_none());
        assert_eq!(
            inspector
                .hovered_element_state::<DivInspectorState>()
                .unwrap()
                .bounds
                .size
                .width,
            px(250.)
        );
    });
    target
        .update(cx, |_, window, cx| {
            assert!(!window.select_inspector_element(child, cx));
            assert!(window.select_inspector_element(parent, cx));
            Inspection::refresh(&inspector, window, cx);
        })
        .unwrap();
    draw(target, cx);
    inspector.read_with(cx, |inspector, _| assert!(!inspector.is_picking()));
}

#[gpui::test]
fn close_reopen_target_release_and_delayed_viewer_update(cx: &mut TestAppContext) {
    let target = setup(cx);
    target
        .update(cx, |_, window, cx| {
            InspectorController::toggle(window, cx).unwrap()
        })
        .unwrap();
    draw(target, cx);
    let old = inspector(cx);
    let weak = old.downgrade();
    cx.update(|cx| {
        // 通知のdeferが残った状態でviewerを閉じても、Targetは生き続ける。
        old.update(cx, |_, cx| cx.notify());
        let viewer = cx
            .global::<InspectorController>()
            .inspector_handle()
            .unwrap();
        viewer
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
    });
    cx.run_until_parked();
    assert!(
        target
            .update(cx, |_, window, cx| !window.is_inspector_picking(cx))
            .unwrap()
    );
    drop(old);
    cx.run_until_parked();
    assert!(weak.upgrade().is_none());
    target
        .update(cx, |_, window, cx| {
            InspectorController::toggle(window, cx).unwrap()
        })
        .unwrap();
    draw(target, cx);
    let viewer = cx.read(|cx| {
        cx.global::<InspectorController>()
            .inspector_handle()
            .unwrap()
    });
    target
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    assert!(viewer.update(cx, |_, _, _| ()).is_err());
}

#[gpui::test]
fn pick_and_wheel_use_target_hit_testing(cx: &mut TestAppContext) {
    let target = setup(cx);
    target
        .update(cx, |_, window, cx| {
            InspectorController::toggle(window, cx).unwrap()
        })
        .unwrap();
    let inspector = inspector(cx);
    inspector.update(cx, |inspector, _| inspector.start_picking());
    draw(target, cx);
    let mut visual = VisualTestContext::from_window(target.into(), cx);
    visual.simulate_mouse_move(point(px(25.), px(25.)), None, Modifiers::none());
    draw(target, cx);
    let first = inspector.read_with(cx, |inspector, _| {
        inspector.active_element_id().cloned().unwrap()
    });
    visual.simulate_event(ScrollWheelEvent {
        position: point(px(25.), px(25.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(36.))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    draw(target, cx);
    let second = inspector.read_with(cx, |inspector, _| {
        inspector.active_element_id().cloned().unwrap()
    });
    assert_ne!(first, second);
    visual.simulate_click(point(px(25.), px(25.)), Modifiers::none());
    draw(target, cx);
    inspector.read_with(cx, |inspector, _| {
        assert!(!inspector.is_picking());
        assert!(
            inspector
                .active_element_state::<DivInspectorState>()
                .is_some()
        );
    });
}

struct CachedTarget(Entity<Target>);
impl Render for CachedTarget {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(gpui::AnyView::from(self.0.clone()).cached(Default::default()))
    }
}

#[gpui::test]
fn idle_inspector_keeps_cached_descendants(cx: &mut TestAppContext) {
    let child = cx.new(|_| Target {
        width: px(200.),
        child: true,
    });
    let target = cx.add_window(|_, _| CachedTarget(child));
    let inspector = cx
        .update_window(target.into(), |_, window, cx| {
            Inspection::enable(window, cx)
        })
        .unwrap();
    cx.update_window(target.into(), |_, window, cx| {
        window.draw(cx).clear();
    })
    .unwrap();
    let revision = inspector.read_with(cx, |inspector, _| {
        assert!(inspector.element_tree().iter().any(|node| {
            node.id
                .path
                .global_id
                .to_string()
                .ends_with("fixture-child")
        }));
        inspector.tree_revision()
    });
    // refreshせずに再描画してもcached subtreeをTreeから落とさない。
    cx.update_window(target.into(), |_, window, cx| {
        window.draw(cx).clear();
    })
    .unwrap();
    inspector.read_with(cx, |inspector, _| {
        assert_eq!(inspector.tree_revision(), revision)
    });
    cx.update_window(target.into(), |_, window, _| Inspection::disable(window))
        .unwrap();
    drop(inspector);
    cx.run_until_parked();
}

#[gpui::test]
fn deferred_old_ui_action_is_rejected_after_reopen(cx: &mut TestAppContext) {
    let target = setup(cx);
    target
        .update(cx, |_, window, cx| {
            InspectorController::toggle(window, cx).unwrap()
        })
        .unwrap();
    let old = inspector(cx);
    let called = std::rc::Rc::new(std::cell::Cell::new(false));
    let captured = called.clone();
    cx.update(|cx| {
        old.update(cx, |_, cx| {
            InspectorController::defer(cx, move |_| captured.set(true))
        });
        cx.update_window(target.into(), |_, window, cx| {
            assert!(!InspectorController::toggle(window, cx).unwrap());
            assert!(InspectorController::toggle(window, cx).unwrap());
        })
        .unwrap();
    });
    cx.run_until_parked();
    assert!(!called.get());
    assert_ne!(old.entity_id(), inspector(cx).entity_id());
}

#[gpui::test]
fn gpui_inspection_state_is_owned_by_each_target(cx: &mut TestAppContext) {
    let first = cx.add_window(|_, _| Target {
        width: px(200.),
        child: true,
    });
    let second = cx.add_window(|_, _| Target {
        width: px(300.),
        child: true,
    });
    let a = cx
        .update_window(first.into(), |_, window, cx| Inspection::enable(window, cx))
        .unwrap();
    let b = cx
        .update_window(second.into(), |_, window, cx| {
            Inspection::enable(window, cx)
        })
        .unwrap();
    draw(first, cx);
    draw(second, cx);
    let id = a.read_with(cx, |inspector, _| inspector.element_tree()[0].id.clone());
    cx.update_window(first.into(), |_, window, cx| {
        assert!(window.select_inspector_element(id.clone(), cx))
    })
    .unwrap();
    cx.update_window(second.into(), |_, window, cx| {
        assert!(!window.select_inspector_element(id, cx))
    })
    .unwrap();
    b.read_with(cx, |inspector, _| {
        assert!(inspector.active_element_id().is_none())
    });
    cx.update_window(first.into(), |_, window, _| Inspection::disable(window))
        .unwrap();
    b.update(cx, |inspector, _| inspector.start_picking());
    assert!(
        second
            .update(cx, |_, window, cx| window.is_inspector_picking(cx))
            .unwrap()
    );
    assert!(
        !first
            .update(cx, |_, window, cx| window.is_inspector_picking(cx))
            .unwrap()
    );
}

struct SourceProbe {
    div: gpui::Div,
    calls: std::rc::Rc<std::cell::Cell<usize>>,
}
impl gpui::Element for SourceProbe {
    type RequestLayoutState = <gpui::Div as gpui::Element>::RequestLayoutState;
    type PrepaintState = <gpui::Div as gpui::Element>::PrepaintState;
    fn id(&self) -> Option<gpui::ElementId> {
        gpui::Element::id(&self.div)
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        self.calls.set(self.calls.get() + 1);
        Some(std::panic::Location::caller())
    }
    fn request_layout(
        &mut self,
        id: Option<&gpui::GlobalElementId>,
        inspector: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut gpui::App,
    ) -> (gpui::LayoutId, Self::RequestLayoutState) {
        self.div.request_layout(id, inspector, window, cx)
    }
    fn prepaint(
        &mut self,
        id: Option<&gpui::GlobalElementId>,
        inspector: Option<&gpui::InspectorElementId>,
        bounds: gpui::Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut gpui::App,
    ) -> Self::PrepaintState {
        self.div.prepaint(id, inspector, bounds, state, window, cx)
    }
    fn paint(
        &mut self,
        id: Option<&gpui::GlobalElementId>,
        inspector: Option<&gpui::InspectorElementId>,
        bounds: gpui::Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut gpui::App,
    ) {
        self.div
            .paint(id, inspector, bounds, state, prepaint, window, cx);
    }
}
impl IntoElement for SourceProbe {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
struct ProbeTarget(std::rc::Rc<std::cell::Cell<usize>>);
impl Render for ProbeTarget {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        SourceProbe {
            div: div().size_full(),
            calls: self.0.clone(),
        }
    }
}

#[gpui::test]
fn disabled_inspector_does_not_collect_element_sources(cx: &mut TestAppContext) {
    let calls = std::rc::Rc::new(std::cell::Cell::new(0));
    let target = cx.add_window(|_, _| ProbeTarget(calls.clone()));
    let draw = |cx: &mut TestAppContext| {
        cx.update_window(target.into(), |_, window, cx| {
            window.refresh();
            window.draw(cx).clear();
        })
        .unwrap();
    };
    draw(cx);
    assert_eq!(calls.get(), 0);
    cx.update_window(target.into(), |_, window, cx| {
        Inspection::enable(window, cx)
    })
    .unwrap();
    draw(cx);
    assert!(calls.get() > 0);
    cx.update_window(target.into(), |_, window, _| Inspection::disable(window))
        .unwrap();
    calls.set(0);
    draw(cx);
    assert_eq!(calls.get(), 0);
}
