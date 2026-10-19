use std::{cell::RefCell, rc::Rc};

use gpui::{
    Context, Inspector, IntoElement, Modifiers, Render, TestAppContext, VisualTestContext,
    WeakEntity, Window, div, prelude::*, px, size,
};

struct Target;

impl Render for Target {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("target")
            .debug_selector(|| "target".into())
            .size_full()
            .p_4()
            .child(
                div()
                    .id("sample")
                    .debug_selector(|| "sample".into())
                    .w(px(120.0))
                    .h(px(80.0))
                    .p_2()
                    .border_1()
                    .child("Sample"),
            )
    }
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    cx.run_until_parked();
}

#[gpui::test]
fn official_inspector_picks_renders_details_and_restores_layout(cx: &mut TestAppContext) {
    let (_, target) = cx.add_window_view(|_, _| Target);
    target.simulate_resize(size(px(1200.0), px(800.0)));
    draw(target);
    let full_width = target.debug_bounds("target").unwrap().size.width;
    // テストだけで公式Entityを観測し、Lapis側には選択状態を保持しない。
    let observed: Rc<RefCell<Option<WeakEntity<Inspector>>>> = Rc::default();
    let output = observed.clone();
    let reserved = target.update(|window, cx| {
        super::init(cx);
        cx.set_inspector_renderer(Box::new(move |inspector, window, cx| {
            *output.borrow_mut() = Some(cx.entity().downgrade());
            super::inspector_view::render_inspector(inspector, window, cx)
        }));
        window.toggle_inspector(cx);
        gpui::rems(30.0).to_pixels(window.rem_size())
    });
    draw(target);
    assert_eq!(
        full_width - target.debug_bounds("target").unwrap().size.width,
        reserved
    );
    assert_eq!(target.update(|_, cx| cx.windows().len()), 1);
    assert_eq!(
        target.debug_bounds("lapis-inspector").unwrap().size.width,
        reserved
    );

    let sample = target.debug_bounds("sample").unwrap();
    // 文字を避け、DivそのものをPickする。
    let point = sample.origin + gpui::point(px(4.0), px(4.0));
    target.simulate_mouse_move(point, None, Modifiers::default());
    target.simulate_click(point, Modifiers::default());
    draw(target);
    let inspector = observed.borrow().as_ref().unwrap().upgrade().unwrap();
    inspector.read_with(target, |inspector, _| {
        assert!(!inspector.is_picking());
        assert!(inspector.active_element_id().is_some());
    });
    assert!(target.debug_bounds("inspector-box-model").is_some());
    assert!(target.debug_bounds("inspector-style").is_some());

    let pick = target.debug_bounds("inspector-pick").unwrap().center();
    target.simulate_mouse_move(pick, None, Modifiers::default());
    target.simulate_click(pick, Modifiers::default());
    draw(target);
    inspector.read_with(target, |inspector, _| assert!(inspector.is_picking()));
    target.simulate_resize(size(px(1000.0), px(700.0)));
    draw(target);
    assert_eq!(
        target.debug_bounds("target").unwrap().size.width,
        px(1000.0) - reserved
    );
    // 公式GPUIはPick中のマウス入力を捕捉する。対象を選択してからCloseを押す。
    let point = target.debug_bounds("sample").unwrap().origin + gpui::point(px(4.0), px(4.0));
    target.simulate_mouse_move(point, None, Modifiers::default());
    target.simulate_click(point, Modifiers::default());
    draw(target);
    inspector.read_with(target, |inspector, _| assert!(!inspector.is_picking()));
    let close = target.debug_bounds("inspector-close").unwrap().center();
    target.simulate_mouse_move(close, None, Modifiers::default());
    target.simulate_click(close, Modifiers::default());
    draw(target);
    assert_eq!(
        target.debug_bounds("target").unwrap().size.width,
        px(1000.0)
    );
    drop(inspector);
    assert!(observed.borrow().as_ref().unwrap().upgrade().is_none());
    target.update(|window, cx| window.toggle_inspector(cx));
    draw(target);
    assert!(target.debug_bounds("lapis-inspector").is_some());
}

#[gpui::test]
fn inspector_lifetime_is_owned_by_each_target_window(cx: &mut TestAppContext) {
    let (_, first) = cx.add_window_view(|_, _| Target);
    first.update(|window, cx| {
        super::init(cx);
        window.toggle_inspector(cx);
    });
    draw(first);
    let first_handle = first.update(|window, _| window.window_handle());
    let (_, second) = first.add_window_view(|_, _| Target);
    draw(second);
    assert!(second.debug_bounds("lapis-inspector").is_none());
    second.update(|window, cx| window.toggle_inspector(cx));
    draw(second);
    let second_handle = second.update(|window, _| window.window_handle());
    first_handle
        .update(second, |_, window, _| window.remove_window())
        .unwrap();
    assert!(second_handle.update(second, |_, _, _| ()).is_ok());
    draw(second);
    assert!(second.debug_bounds("lapis-inspector").is_some());
    second.update(|window, cx| window.toggle_inspector(cx));
    draw(second);
    let full_width = second.update(|window, _| window.viewport_size().width);
    assert_eq!(
        second.debug_bounds("target").unwrap().size.width,
        full_width
    );
}
