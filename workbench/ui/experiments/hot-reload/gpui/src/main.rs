use gpui::{prelude::*, *};
use std::sync::Arc;

struct Probe {
    typed: Entity<TypedView>,
    erased: AnyView,
}

struct TypedView;
struct ErasedView;

impl Render for TypedView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        eprintln!("typed render pid={}", std::process::id());
        div().child("typed view")
    }
}

impl Render for ErasedView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        eprintln!("any view render pid={}", std::process::id());
        div().child("any view")
    }
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        eprintln!("render pid={}", std::process::id());
        div()
            .size_full()
            .bg(rgb(0x4c3810))
            .child(self.typed.clone())
            .child(self.erased.clone())
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
        cx.open_window(WindowOptions::default(), |_, cx| {
            cx.new(|cx| Probe {
                typed: cx.new(|_| TypedView),
                erased: cx.new(|_| ErasedView).into(),
            })
        })
        .unwrap();
        cx.activate(true);
    });
}
