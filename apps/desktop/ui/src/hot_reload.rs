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
    // dioxus_devtools::connect_subsecond() は内部で接続用スレッドをspawnするが、
    // 接続完了/切断を呼び出し元に通知しない。そのため、Lapis側で接続状態を
    // 明示的に監視し、切断後に再接続する。
    //
    // 初回接続は即座に試みる。接続成功後はdevserverのWebSocketポートを定期的に
    // 確認し、切断を検知したら指数バックオフで再接続する。
    // 無条件にconnect_subsecond()を繰り返すと接続スレッドが増殖するため避ける。
    cx.spawn(async move |cx| {
        let mut delay_secs: u64 = RECONNECT_INITIAL_SECS;
        let mut connected = false;

        loop {
            if !connected {
                dioxus_devtools::connect_subsecond();
                eprintln!(
                    "hot-reload connecting to devserver pid={}",
                    std::process::id()
                );

                // 接続試行後に少し待って、WebSocket接続が確立されるのを待つ
                async_sleep(std::time::Duration::from_millis(500)).await;

                // devserverが応答しているか確認
                if check_devserver() {
                    connected = true;
                    eprintln!("hot-reload devserver connected pid={}", std::process::id());
                    delay_secs = RECONNECT_INITIAL_SECS;
                }
            }

            // 再接続間隔待機
            async_sleep(std::time::Duration::from_secs(delay_secs)).await;

            // アプリケーションが終了していたらループを抜ける
            if cx.update(|cx| cx.windows().is_empty()).unwrap_or(true) {
                break;
            }

            // devserverの接続状態を確認
            if !check_devserver() {
                if connected {
                    // 接続が切れた
                    connected = false;
                    eprintln!(
                        "hot-reload devserver disconnected pid={}",
                        std::process::id()
                    );
                }
                // バックオフを増やす
                delay_secs = (delay_secs * 2).min(RECONNECT_MAX_SECS);
                eprintln!(
                    "hot-reload reconnecting in {}s pid={}",
                    delay_secs,
                    std::process::id()
                );
            }
        }
    })
    .detach();
} // init

/// devserver が応答しているか確認する。
/// WebSocketポート（デフォルト 4243）にTCP接続できるか試す。
///
/// dioxus_devtoolsの内部状態に直接アクセスできないため、
/// TCPポートの接続でdevserverの生存を確認する。
fn check_devserver() -> bool {
    use std::net::{SocketAddr, TcpStream, ToSocketAddrs};

    let addr = match ("127.0.0.1", 4243).to_socket_addrs() {
        Ok(mut addrs) => addrs.next(),
        Err(_) => return false,
    };

    match addr {
        Some(addr) => {
            let socket_addr = SocketAddr::new(addr.ip(), addr.port());
            TcpStream::connect_timeout(&socket_addr, std::time::Duration::from_millis(100)).is_ok()
        }
        None => false,
    }
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
