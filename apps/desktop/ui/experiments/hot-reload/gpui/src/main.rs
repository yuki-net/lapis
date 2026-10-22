use gpui::{prelude::*, *};
use std::sync::Arc;

struct Probe;

impl Render for Probe {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        subsecond::HotFn::current(Self::render_hot).call((self, window, cx))
    }
}

impl Probe {
    #[inline(never)]
    fn render_hot(&mut self, _: &mut Window, _: &mut Context<Self>) -> AnyElement {
        eprintln!("render pid={}", std::process::id());
        div().size_full().bg(rgb(0x4c3810)).into_any_element()
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        eprintln!("probe-start pid={}", std::process::id());
        let (sender, receiver) = async_channel::bounded(1);
        subsecond::register_handler(Arc::new(move || {
            eprintln!("patch-applied pid={}", std::process::id());
            let _ = sender.try_send(());
        }));
        cx.spawn(async move |cx| {
            while receiver.recv().await.is_ok() {
                if cx
                    .update(|cx| {
                        eprintln!("refresh windows={}", cx.windows().len());
                        cx.refresh_windows();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        dioxus_devtools::connect_subsecond();
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| Probe))
            .unwrap();
        cx.activate(true);
    });
}
