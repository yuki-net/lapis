//! 明示的なWindow Host smokeモードだけでInspectorのNative描画と寿命を検証する。
use super::*;
use gpui::{AsyncApp, Inspector, Timer};
use std::time::{Duration, Instant};
use windows::Win32::UI::WindowsAndMessaging::{WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE};

fn sample_frames(target: AnyWindowHandle, phase: &str, cx: &mut AsyncApp) {
    let start = Instant::now();
    for _ in 0..10 {
        target
            .update(cx, |_, window, cx| window.draw(cx))
            .unwrap()
            .clear();
    }
    // GPU presentを含まないフレーム構築時間。継続負荷の測定ではない。
    eprintln!(
        "SMOKE Inspector {phase}: frame construction {:.3} ms/frame",
        start.elapsed().as_secs_f64() * 100.0
    );
}

pub(crate) async fn run(target: AnyWindowHandle, cx: &mut AsyncApp) {
    sample_frames(target, "disabled", cx);
    let width = target
        .update(cx, |_, window, _| window.viewport_size().width)
        .unwrap();
    target
        .update(cx, |_, window, cx| {
            assert_eq!(InspectorController::toggle(window, cx), Ok(true))
        })
        .unwrap();
    Timer::after(Duration::from_millis(250)).await;
    let viewer = cx
        .update(|cx| cx.global::<InspectorController>().inspector_window.unwrap())
        .unwrap();
    assert!(viewer != target);
    let inspector = viewer.downcast::<Inspector>().unwrap().root(cx).unwrap();
    let weak = inspector.downgrade();
    let node = inspector
        .read_with(cx, |inspector, _| {
            assert!(inspector.element_tree().len() > 10);
            inspector
                .element_tree()
                .iter()
                .find(|node| {
                    node.element_type.contains("div::Div")
                        && node.bounds.size.width > px(30.0)
                        && node.bounds.size.height > px(30.0)
                })
                .unwrap()
                .clone()
        })
        .unwrap();
    target
        .update(cx, |_, window, cx| {
            assert_eq!(window.viewport_size().width, width);
            assert!(window.select_inspector_element(node.id.clone(), cx));
        })
        .unwrap();
    Timer::after(Duration::from_millis(150)).await;
    inspector
        .read_with(cx, |inspector, _| {
            assert_eq!(inspector.active_element_id(), Some(&node.id));
            assert!(
                inspector
                    .active_element_state::<gpui::DivInspectorState>()
                    .is_some()
            );
        })
        .unwrap();
    target
        .update(cx, |_, window, cx| {
            assert!(window.preview_inspector_element(Some(node.id.clone()), cx))
        })
        .unwrap();
    sample_frames(target, "enabled", cx);
    eprintln!("SMOKE Inspector sample begin");
    Timer::after(Duration::from_secs(3)).await;
    inspector
        .update(cx, |inspector, _| inspector.start_picking())
        .unwrap();
    target.update(cx, |_, window, _| window.refresh()).unwrap();
    Timer::after(Duration::from_millis(150)).await;
    let point = node.bounds.origin + gpui::point(px(4.0), px(4.0));
    post_mouse(target, point, WM_MOUSEMOVE, cx);
    Timer::after(Duration::from_millis(150)).await;
    sample_frames(target, "picking", cx);
    eprintln!("SMOKE Pick sample begin");
    Timer::after(Duration::from_secs(3)).await;
    post_mouse(target, point, WM_LBUTTONDOWN, cx);
    post_mouse(target, point, WM_LBUTTONUP, cx);
    Timer::after(Duration::from_millis(150)).await;
    inspector
        .read_with(cx, |inspector, _| {
            assert!(!inspector.is_picking());
            assert!(inspector.active_element_id().is_some());
        })
        .unwrap();
    viewer
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
    drop(inspector);
    Timer::after(Duration::from_millis(150)).await;
    assert!(weak.upgrade().is_none());
    target
        .update(cx, |_, window, cx| {
            assert_eq!(InspectorController::toggle(window, cx), Ok(true))
        })
        .unwrap();
    target
        .update(cx, |_, window, cx| {
            assert_eq!(InspectorController::toggle(window, cx), Ok(false))
        })
        .unwrap();
    eprintln!("SMOKE Inspector tree, selection, hover, pick, release and reopen OK");
}

fn post_mouse(
    target: AnyWindowHandle,
    point: gpui::Point<gpui::Pixels>,
    message: u32,
    cx: &mut AsyncApp,
) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        UI::WindowsAndMessaging::PostMessageW,
    };
    target
        .update(cx, |_, window, _| {
            let RawWindowHandle::Win32(raw) =
                HasWindowHandle::window_handle(window).unwrap().as_raw()
            else {
                panic!("Windows handle required");
            };
            let scale = window.scale_factor();
            let x = (f32::from(point.x) * scale).round() as i16 as u16 as u32;
            let y = (f32::from(point.y) * scale).round() as i16 as u16 as u32;
            // 明示的なsmokeの対象Windowだけに送信する。
            unsafe {
                PostMessageW(
                    Some(HWND(raw.hwnd.get() as *mut _)),
                    message,
                    WPARAM(usize::from(message == WM_LBUTTONDOWN)),
                    LPARAM(((y << 16) | x) as isize),
                )
            }
            .unwrap();
        })
        .unwrap();
}
