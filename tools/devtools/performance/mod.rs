//! Performance 診断ログ機能。
//!
//! Lapis / GPUI のUI性能を記録し、DevTool Performance タブで表示する。

pub(crate) mod entry;
pub(crate) mod store;

use gpui::App;

/// Performance 診断ログ store を初期化する。
pub(crate) fn init(cx: &mut App) {
    store::init(cx);
}
