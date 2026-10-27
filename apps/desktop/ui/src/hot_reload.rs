use std::sync::{Arc, OnceLock};

use gpui::App;

/// devserver 再接続の初期間隔（秒）。指数バックオフの起点。
const RECONNECT_INITIAL_SECS: u64 = 2;

/// devserver 再接続の最大間隔（秒）。
const RECONNECT_MAX_SECS: u64 = 30;

/// Subsecond の通知を GPUI の foreground executor へ渡し、既存 Window を再描画する。
///
/// devserver が起動していない場合でもプロセスを終了せず、
/// 非同期で再接続を試みる（指数バックオフ）。
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

    // patch 適用通知 → 既存 Window の再描画
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

    // devserver 接続と再接続
    // dioxus_devtools::connect_subsecond() は初回呼び出しで接続を試みる。
    // 切断後は再度呼び出すことで再接続を試みる。
    cx.spawn(async move |cx| {
        let mut delay_secs: u64 = RECONNECT_INITIAL_SECS;
        loop {
            dioxus_devtools::connect_subsecond();
            eprintln!("hot-reload devserver connected pid={}", std::process::id());

            // 再接続間隔待機
            async_sleep(std::time::Duration::from_secs(delay_secs)).await;
            delay_secs = RECONNECT_INITIAL_SECS;

            // アプリケーションが終了していたらループを抜ける
            if cx.update(|cx| cx.windows().is_empty()).unwrap_or(true) {
                break;
            }
            delay_secs = (delay_secs * 2).min(RECONNECT_MAX_SECS);
            eprintln!(
                "hot-reload reconnecting in {}s pid={}",
                delay_secs,
                std::process::id()
            );
        }
    })
    .detach();
}

/// tokioに依存しない非同期sleep。async_channelのrecv_timeoutを使用する。
async fn async_sleep(duration: std::time::Duration) {
    let (tx, rx) = async_channel::bounded(1);
    // spawn_blockingでブロックsleepし、完了後にchannelで通知
    std::thread::spawn(move || {
        std::thread::sleep(duration);
        let _ = tx.send(());
    });
    let _ = rx.recv().await;
}
