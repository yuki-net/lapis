//! DevTool Console タブ。
//!
//! tracing ベースの診断ログを収集し、DevTool Console で表示する。

pub(crate) mod entry;
mod layer;
pub(crate) mod store;
pub(crate) mod view;

pub(crate) use layer::init_subscriber;
pub(crate) use store::init;
