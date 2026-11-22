//! Workspace を開いている Session 間で view state を共有するための port。
//!
//! 正本は Host にあり、購読した Client は snapshot の後に最新の view 全体を
//! `SessionViewEvent` として受け取る。差分計算やマージは行わず、最後に受理した値が
//! そのまま配られる。読取り権限を持たない Session へは event の `view` が `None` と
//! なるため、Client はそれを適用しない。

use std::time::Duration;

use editor_core::WorkspaceId;

use crate::WorkspaceView;

/// 購読開始時点の view state。まだ誰も共有していない Workspace は `view` が `None`。
#[derive(Clone, Debug, PartialEq)]
pub struct SessionViewSnapshot {
    pub revision: u64,
    pub view: Option<WorkspaceView>,
}

/// 共有中の view state の置き換え。`view` が `None` の event は権限で伏せられている。
#[derive(Clone, Debug, PartialEq)]
pub struct SessionViewEvent {
    pub revision: u64,
    pub view: Option<WorkspaceView>,
}

/// Workspace 単位の view state 共有の port。
///
/// `subscribe` は共有への参加と現在値の取得を兼ねるため、再同期でも同じ呼び出しを使う。
/// `update` は view 全体を送り、Host が直列化して受理した revision を返す。
pub trait SessionViewConnection: Send + Sync {
    fn subscribe(&self, workspace: &WorkspaceId) -> Result<SessionViewSnapshot, String>;
    fn unsubscribe(&self, workspace: &WorkspaceId) -> Result<(), String>;
    fn update(&self, workspace: &WorkspaceId, view: WorkspaceView) -> Result<u64, String>;
    /// 接続中の全 Session に配られる `workspace.view_changed` を受け取る。
    /// 未購読の間も event は届くため、適用可否は購読状態を持つ側で判断する。
    fn events(&self, workspace: &WorkspaceId) -> Result<Box<dyn SessionViewEvents>, String>;
}

/// `workspace.view_changed` の受信待ち。timeout は待ち続ける上限を表す。
pub trait SessionViewEvents: Send {
    fn next(&mut self, timeout: Duration) -> Result<Option<SessionViewEvent>, String>;
}
