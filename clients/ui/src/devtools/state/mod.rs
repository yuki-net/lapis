//! State 診断機能。
//!
//! Lapis 内部の現在状態を DevTool State タブで読み取り専用で確認する。

pub(crate) mod snapshot;
pub(crate) mod store;

use gpui::App;

/// State 診断 store を初期化する。
pub(crate) fn init(cx: &mut App) {
    store::init(cx);
}
