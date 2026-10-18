use gpui::{
    AnyWindowHandle, App, AppContext, Application, Bounds, Context, DivInspectorState, Entity,
    Inspector, IntoElement, Render, TitlebarOptions, WeakEntity, Window, WindowBounds,
    WindowOptions, div, prelude::*, px, rgb, size,
};

// 対象ウィンドウの状態へのアクセスと、DevToolsの表示を分ける最小構成。
struct InspectorSession {
    target: AnyWindowHandle,
    inspector: Option<WeakEntity<Inspector>>,
}

struct DevTools {
    session: Entity<InspectorSession>,
}

impl Render for DevTools {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let session = self.session.read(cx);
        let target = session.target;
        let inspector = session.inspector.as_ref().and_then(WeakEntity::upgrade);
        div()
            .size_full()
            .flex()
            .flex_col()
            .p_4()
            .gap_3()
            .bg(rgb(0x20242c))
            .text_color(rgb(0xeeeeee))
            .child("DevTools / Inspector (official GPUI)")
            .child("Target側の30remの領域は、空のrendererでも残ります。")
            .when_some(inspector, |element, inspector| {
                element.child(inspector.update(cx, |inspector, cx| {
                    let selected = inspector.active_element_id().map(|id| {
                        let source = id.path.source_location;
                        format!(
                            "{} · {}:{}:{} · instance {}",
                            id.path.global_id,
                            source.file(),
                            source.line(),
                            source.column(),
                            id.instance_id,
                        )
                    });
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div()
                                .id("pick")
                                .p_2()
                                .bg(rgb(0x375375))
                                .cursor_pointer()
                                .child(if inspector.is_picking() {
                                    "Picking: Targetでクリック / ホイールで深さ変更"
                                } else {
                                    "Pick element"
                                })
                                .on_click(cx.listener(move |inspector, _, _, cx| {
                                    inspector.start_picking();
                                    cx.notify();
                                    // 別ウィンドウの更新は描画中に再入しないようdeferする。
                                    cx.defer(move |cx| {
                                        let _ = target.update(cx, |_, window, _| window.refresh());
                                    });
                                })),
                        )
                        .child(selected.unwrap_or_else(|| "要素を選択してください".into()))
                        .children(inspector.render_inspector_states(window, cx))
                }))
            })
    }
}

struct Target;

impl Render for Target {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("target-root")
            .debug_selector(|| "target-root".into())
            .size_full()
            .flex()
            .flex_col()
            .p_4()
            .gap_4()
            .bg(rgb(0x282c34))
            .text_color(rgb(0xeeeeee))
            .child("Target / official GPUI 0.2.2")
            .child(
                div()
                    .id("sample")
                    .debug_selector(|| "sample".into())
                    .p_4()
                    .border_2()
                    .border_color(rgb(0x61afef))
                    .bg(rgb(0x334455))
                    .child("このDivをPickすると別ウィンドウにBoundsとStyleが表示されます"),
            )
    }
}

fn install_bridge(session: &Entity<InspectorSession>, cx: &mut App) {
    let target = session.read(cx).target;
    let session = session.downgrade();
    cx.set_inspector_renderer(Box::new(move |_, window, cx| {
        if window.window_handle() == target {
            let inspector = cx.entity().downgrade();
            let session = session.clone();
            cx.defer(move |cx| {
                let _ = session.update(cx, |session, cx| {
                    session.inspector = Some(inspector);
                    cx.notify();
                });
            });
        }
        // Emptyでも対象のroot幅は復元されない。制約を可視化する。
        div()
            .size_full()
            .p_4()
            .bg(rgb(0x482f35))
            .text_color(rgb(0xffffff))
            .child("GPUI予約領域 (30rem): 公開APIでは外せません")
            .into_any_element()
    }));
    cx.register_inspector_element(|_, state: &DivInspectorState, _, _| {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(format!("Bounds: {:?}", state.bounds))
            .child(format!("Content: {:?}", state.content_size))
            .child(format!("margin: {:?}", state.base_style.margin))
            .child(format!("border: {:?}", state.base_style.border_widths))
            .child(format!("padding: {:?}", state.base_style.padding))
            .child(format!("background: {:?}", state.base_style.background))
    });
}

fn options(title: &'static str, width: f32, x: f32) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            gpui::point(px(x), px(60.0)),
            size(px(width), px(650.0)),
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some(title.into()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn main() {
    Application::new().run(|cx| {
        let target = cx
            .open_window(options("GPUI PoC Target", 1100.0, 30.0), |_, cx| {
                cx.new(|_| Target)
            })
            .expect("Target window");
        let session = cx.new(|_| InspectorSession {
            target: target.into(),
            inspector: None,
        });
        install_bridge(&session, cx);
        target
            .update(cx, |_, window, cx| window.toggle_inspector(cx))
            .expect("Enable inspector");
        let viewer = cx
            .open_window(options("GPUI PoC DevTools", 720.0, 800.0), |_, cx| {
                cx.new(|_| DevTools { session })
            })
            .expect("DevTools window");
        cx.on_window_closed(move |cx| {
            let windows = cx.windows();
            if !windows.contains(&target.into()) {
                let _ = viewer.update(cx, |_, window, _| window.remove_window());
                cx.quit();
            } else if !windows.contains(&viewer.into()) {
                let _ = target.update(cx, |_, window, cx| window.toggle_inspector(cx));
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Modifiers, TestAppContext, VisualTestContext};
    use std::{cell::RefCell, rc::Rc};

    fn draw(window: &mut VisualTestContext) {
        window.update(|window, cx| {
            let _ = window.draw(cx);
        });
        window.run_until_parked();
    }

    #[gpui::test]
    fn empty_renderer_still_reserves_target_width(cx: &mut TestAppContext) {
        let (_, target) = cx.add_window_view(|_, _| Target);
        draw(target);
        let full_width = target.debug_bounds("target-root").unwrap().size.width;
        let reserved = target.update(|window, cx| {
            cx.set_inspector_renderer(Box::new(|_, _, _| gpui::Empty.into_any_element()));
            window.toggle_inspector(cx);
            gpui::rems(30.0).to_pixels(window.rem_size())
        });
        draw(target);
        let inspected_width = target.debug_bounds("target-root").unwrap().size.width;
        assert_eq!(full_width - inspected_width, reserved);

        target.update(|window, cx| window.toggle_inspector(cx));
        draw(target);
        assert_eq!(
            target.debug_bounds("target-root").unwrap().size.width,
            full_width
        );
    }

    #[gpui::test]
    fn explicit_viewport_width_can_override_root_allocation(cx: &mut TestAppContext) {
        struct ExplicitRoot;
        impl Render for ExplicitRoot {
            fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
                    .id("explicit-root")
                    .debug_selector(|| "explicit-root".into())
                    .w(window.viewport_size().width)
                    .h_full()
            }
        }
        let (_, target) = cx.add_window_view(|_, _| ExplicitRoot);
        target.update(|window, cx| {
            cx.set_inspector_renderer(Box::new(|_, _, _| gpui::Empty.into_any_element()));
            window.toggle_inspector(cx);
        });
        draw(target);
        let width = target.debug_bounds("explicit-root").unwrap().size.width;
        target.update(|window, _| assert_eq!(width, window.viewport_size().width));
    }

    #[gpui::test]
    fn picked_div_state_can_render_in_separate_window(cx: &mut TestAppContext) {
        let (_, target) = cx.add_window_view(|_, _| Target);
        let session = target.update(|window, cx| {
            let session = cx.new(|_| InspectorSession {
                target: window.window_handle(),
                inspector: None,
            });
            install_bridge(&session, cx);
            window.toggle_inspector(cx);
            session
        });
        draw(target);
        let sample = target.debug_bounds("sample").unwrap();
        target.simulate_mouse_move(sample.center(), None, Modifiers::default());
        target.simulate_click(sample.center(), Modifiers::default());
        draw(target);
        let inspector = session.read_with(target, |session, _| {
            session.inspector.as_ref().unwrap().upgrade().unwrap()
        });
        inspector.read_with(target, |inspector, _| {
            assert!(!inspector.is_picking());
            assert!(inspector.active_element_id().is_some());
        });

        let rendered = Rc::new(RefCell::new(None));
        let output = rendered.clone();
        target.update(|_, cx| {
            cx.register_inspector_element(move |_, state: &DivInspectorState, window, _| {
                *output.borrow_mut() = Some((state.bounds, window.window_handle()));
                div()
            });
        });
        let session_for_viewer = session.clone();
        // VisualTestContextから元のTestAppContextへアクセスする。
        let (_, viewer) = target.add_window_view(move |_, _| DevTools {
            session: session_for_viewer,
        });
        draw(viewer);
        let (bounds, rendered_in) = rendered.borrow().expect("Div state rendered in viewer");
        assert_eq!(bounds, sample);
        viewer.update(|window, _| assert!(rendered_in == window.window_handle()));

        inspector.update(viewer, |inspector, _| inspector.start_picking());
        inspector.read_with(viewer, |inspector, _| assert!(inspector.is_picking()));
        // 閉じた対象へのdefer先はWindowHandleのエラーとして扱える。
        let target_handle = session.read_with(viewer, |session, _| session.target);
        target_handle
            .update(viewer, |_, window, _| window.remove_window())
            .unwrap();
        assert!(target_handle.update(viewer, |_, _, _| ()).is_err());
    }
}
