//! wasm には std の時刻 API がないため、ブラウザーでは `web-time` を使う。
pub use web_time::{Duration, Instant, SystemTime};
