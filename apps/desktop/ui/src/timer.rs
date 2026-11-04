use std::time::Duration;

/// 各platformの非同期タイマーを共通の呼び出しへ揃える。
pub(crate) struct Timer;

impl Timer {
    pub(crate) async fn after(duration: Duration) {
        #[cfg(not(target_family = "wasm"))]
        gpui::Timer::after(duration).await;
        #[cfg(target_family = "wasm")]
        gloo_timers::future::TimeoutFuture::new(duration.as_millis().min(i32::MAX as u128) as u32)
            .await;
    }
}
