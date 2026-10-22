use std::sync::{Arc, OnceLock};

use gpui::App;

/// Subsecond の通知を GPUI の foreground executor へ渡し、既存 Window を再描画する。
pub(crate) fn init(cx: &mut App) {
    eprintln!("hot-reload init pid={}", std::process::id());
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }

    let (sender, receiver) = async_channel::bounded(1);
    subsecond::register_handler(Arc::new(move || {
        eprintln!("hot-reload patch-applied pid={}", std::process::id());
        let _ = sender.try_send(());
    }));

    cx.spawn(async move |cx| {
        while receiver.recv().await.is_ok() {
            if cx
                .update(|cx| {
                    eprintln!("hot-reload refresh windows={}", cx.windows().len());
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
}
