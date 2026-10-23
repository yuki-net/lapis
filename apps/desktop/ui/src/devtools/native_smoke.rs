//! 明示的なNative smokeだけで動かすInspector検証と短時間の性能サンプル。
use super::{inspection::Inspection, inspector_controller::InspectorController};
use gpui::{AnyWindowHandle, AsyncApp, Timer};
use std::time::{Duration, Instant};

async fn sample(name: &str, target: AnyWindowHandle, cx: &mut AsyncApp) {
    let mut total = Duration::ZERO;
    for _ in 0..30 {
        total += target
            .update(cx, |_, window, cx| {
                let start = Instant::now();
                window.refresh();
                window.draw(cx).clear();
                start.elapsed()
            })
            .unwrap();
        Timer::after(Duration::from_millis(16)).await;
    }
    eprintln!(
        "INSPECTOR frame {name}: {:.3} ms (CPU draw, 30 samples)",
        total.as_secs_f64() * 1000. / 30.
    );
    eprintln!("INSPECTOR sample {name} begin");
    Timer::after(Duration::from_secs(3)).await;
}

pub(crate) async fn run(target: AnyWindowHandle, cx: &mut AsyncApp) {
    sample("normal", target, cx).await;
    let before = target
        .update(cx, |_, window, cx| {
            let size = window.viewport_size();
            assert!(InspectorController::toggle(window, cx).unwrap());
            size
        })
        .unwrap();
    sample("visible", target, cx).await;
    #[cfg(target_os = "windows")]
    {
        let viewer = cx
            .update(|cx| {
                cx.global::<InspectorController>()
                    .inspector_handle(target.window_id())
                    .unwrap()
            })
            .unwrap();
        check_toolbar_hit_test(viewer, cx);
    }
    let (viewer, inspector, selected) = target
        .update(cx, |_, window, cx| {
            assert_eq!(before, window.viewport_size());
            let viewer = cx
                .global::<InspectorController>()
                .inspector_handle(target.window_id())
                .unwrap();
            let inspector = cx
                .global::<InspectorController>()
                .inspection_entity(target.window_id())
                .unwrap();
            let id = inspector
                .read(cx)
                .element_tree()
                .iter()
                .find(|node| node.element_type == std::any::type_name::<gpui::Div>())
                .unwrap()
                .id
                .clone();
            assert!(window.select_inspector_element(id.clone(), cx));
            assert!(window.preview_inspector_element(Some(id.clone()), cx));
            // Hiddenで起動するNative検証でも検査stateの描画更新を完了させる。
            window.draw(cx).clear();
            (viewer, inspector, id)
        })
        .unwrap();
    Timer::after(Duration::from_millis(200)).await;
    target
        .update(cx, |_, window, cx| {
            assert_eq!(inspector.read(cx).active_element_id(), Some(&selected));
            assert!(
                inspector
                    .read(cx)
                    .active_element_state::<gpui::DivInspectorState>()
                    .is_some()
            );
            window.preview_inspector_element(None, cx);
            inspector.update(cx, |inspector, _| inspector.start_picking());
            window.refresh();
        })
        .unwrap();
    sample("picking", target, cx).await;
    let weak = inspector.downgrade();
    drop(inspector);
    viewer
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
    Timer::after(Duration::from_millis(200)).await;
    assert!(weak.upgrade().is_none());
    target
        .update(cx, |_, window, cx| {
            assert!(!window.is_inspector_picking(cx));
            assert!(InspectorController::toggle(window, cx).unwrap());
        })
        .unwrap();
    Timer::after(Duration::from_millis(200)).await;
    target
        .update(cx, |_, window, cx| {
            assert!(!InspectorController::toggle(window, cx).unwrap());
            Inspection::disable(window);
        })
        .unwrap();
    eprintln!("INSPECTOR_NATIVE_SMOKE_PASS");
}

#[cfg(target_os = "windows")]
fn check_toolbar_hit_test(viewer: AnyWindowHandle, cx: &mut AsyncApp) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        UI::WindowsAndMessaging::{HTCLIENT, SendMessageW, WM_NCHITTEST},
    };
    let (raw, points) = viewer
        .update(cx, |_, window, cx| {
            let inspection = Inspection::enable(window, cx);
            window.draw(cx).clear();
            let points = ["inspector-refresh", "inspector-pick"].map(|id| {
                let bounds = inspection
                    .read(cx)
                    .element_tree()
                    .iter()
                    .find(|node| node.id.path.global_id.to_string().contains(id))
                    .unwrap()
                    .bounds;
                crate::window_host::drag::screen_position(bounds.center(), window).unwrap()
            });
            Inspection::disable(window);
            let RawWindowHandle::Win32(raw) =
                HasWindowHandle::window_handle(window).unwrap().as_raw()
            else {
                panic!("Windows handle required")
            };
            (raw.hwnd.get(), points)
        })
        .unwrap();
    // Windowを借用していない位置で、OSからGPUIのnative Hit Testを呼ぶ。
    for point in points {
        let coordinates =
            LPARAM(((point.y as i16 as u16 as u32) << 16 | point.x as i16 as u16 as u32) as isize);
        let result = unsafe {
            SendMessageW(
                HWND(raw as *mut _),
                WM_NCHITTEST,
                Some(WPARAM(0)),
                Some(coordinates),
            )
        };
        assert_eq!(
            result.0, HTCLIENT as isize,
            "Inspector toolbar must not be a caption drag area"
        );
    }
    eprintln!("INSPECTOR_NATIVE_TOOLBAR_PASS");
}

pub(crate) fn open_for_target(target: AnyWindowHandle, cx: &mut AsyncApp) -> AnyWindowHandle {
    target
        .update(cx, |_, window, cx| {
            InspectorController::open(window, cx).unwrap()
        })
        .unwrap()
}

/// 実際のRoot / Detachedを同時検査し、開閉してもTargetの所属に触れない。
pub(crate) async fn check_pair(root: AnyWindowHandle, child: AnyWindowHandle, cx: &mut AsyncApp) {
    let a_viewer = open_for_target(root, cx);
    assert_eq!(open_for_target(root, cx).window_id(), a_viewer.window_id());
    let b_viewer = open_for_target(child, cx);
    assert_ne!(a_viewer.window_id(), b_viewer.window_id());
    for target in [root, child] {
        target
            .update(cx, |_, window, cx| {
                window.refresh();
                window.draw(cx).clear();
            })
            .unwrap();
    }
    let (a, b) = cx
        .update(|cx| {
            let controller = cx.global::<InspectorController>();
            (
                controller.inspection_entity(root.window_id()).unwrap(),
                controller.inspection_entity(child.window_id()).unwrap(),
            )
        })
        .unwrap();
    cx.update(|cx| {
        assert_ne!(a.entity_id(), b.entity_id());
        let id = a.read(cx).element_tree().first().unwrap().id.clone();
        InspectorController::select_element(id.clone(), a.entity_id(), cx);
        InspectorController::hover_element(Some(id), a.entity_id(), cx);
        assert!(b.read(cx).active_element_id().is_none());
        assert!(b.read(cx).hovered_element_id().is_none());
        b.update(cx, |inspector, _| inspector.start_picking());
    })
    .unwrap();
    root.update(cx, |_, window, cx| {
        assert!(!window.is_inspector_picking(cx))
    })
    .unwrap();
    child
        .update(cx, |_, window, cx| assert!(window.is_inspector_picking(cx)))
        .unwrap();
    a_viewer
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
    Timer::after(Duration::from_millis(100)).await;
    assert!(root.update(cx, |_, _, _| ()).is_ok());
    assert!(b_viewer.update(cx, |_, _, _| ()).is_ok());
    let reopened = open_for_target(root, cx);
    assert_ne!(reopened.window_id(), a_viewer.window_id());
    for viewer in [reopened, b_viewer] {
        viewer
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
    }
    Timer::after(Duration::from_millis(100)).await;
    for target in [root, child] {
        target
            .update(cx, |_, window, cx| {
                assert!(!window.is_inspector_picking(cx))
            })
            .unwrap();
    }
    eprintln!("INSPECTOR_NATIVE_PAIR_PASS");
}
