//! Network 診断ログ機能。
//!
//! Lapis の論理的な通信操作（Request/Response/Event）を観測し、
//! DevTool Network タブで表示する。

pub(crate) mod entry;
pub(crate) mod store;

use gpui::App;

/// Network 診断ログ store を初期化する。
pub(crate) fn init(cx: &mut App) {
    store::init(cx);
}
