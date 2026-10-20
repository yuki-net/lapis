use std::time::{SystemTime, UNIX_EPOCH};

/// 認証の期限判定に使う時刻の境界。
pub trait Clock {
    fn now_unix_seconds(&self) -> u64;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_unix_seconds(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs())
    }
}
