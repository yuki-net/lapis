//! Web entry から共通 UI へ渡す接続処理の差し込み口。
//!
//! `desktop-ui` は Web の Host 接続実装を持たない。wasm entry が起動時にこの bridge を
//! 登録し、Workspace 切替や URL 通知だけが同じ実装を呼べるようにする。

use std::sync::OnceLock;

use desktop_client::EditorSession;

use crate::DesktopServices;

#[derive(Clone, Copy)]
pub struct WebBridge {
    /// Workspace 切替後に UI を組み直すための Session 一式を作る。
    pub host_sessions: fn() -> Result<(EditorSession, DesktopServices), String>,
    /// URL fragment の Task 選択をページへ通知する。
    pub task_selected: fn(&str),
    /// Workspace 切替後の接続情報をページへ通知する。
    pub workspace_switched: fn(),
}

static BRIDGE: OnceLock<WebBridge> = OnceLock::new();

/// Web entry の起動時に 1 度だけ登録する。
pub fn install(bridge: WebBridge) {
    let _ = BRIDGE.set(bridge);
}

/// 登録済みの接続処理から Session 一式を作る。
pub fn host_sessions() -> Result<(EditorSession, DesktopServices), String> {
    let bridge = BRIDGE
        .get()
        .ok_or_else(|| "Web の起動情報が登録されていません".to_owned())?;
    (bridge.host_sessions)()
}

pub fn notify_task_selected(task: &str) {
    if let Some(bridge) = BRIDGE.get() {
        (bridge.task_selected)(task);
    }
}

pub fn notify_workspace_switched() {
    if let Some(bridge) = BRIDGE.get() {
        (bridge.workspace_switched)();
    }
}
