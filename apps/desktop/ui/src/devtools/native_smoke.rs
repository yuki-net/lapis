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
    let (viewer, inspector, selected) = target
        .update(cx, |_, window, cx| {
            assert_eq!(before, window.viewport_size());
            let viewer = cx
                .global::<InspectorController>()
                .inspector_handle()
                .unwrap();
            let inspector = viewer
                .downcast::<gpui::Inspector>()
                .unwrap()
                .root(cx)
                .unwrap();
            let id = inspector
                .read(cx)
                .element_tree()
                .iter()
                .find(|node| node.element_type.contains("::div::Div"))
                .unwrap()
                .id
                .clone();
            assert!(window.select_inspector_element(id.clone(), cx));
            assert!(window.preview_inspector_element(Some(id.clone()), cx));
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
