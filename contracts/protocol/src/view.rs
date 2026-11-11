//! Workspace の view state（開いている文書と表示位置）の session 共有契約。
//!
//! 正本は Host の Workspace session にあり、購読した Client は snapshot の後に
//! `workspace.view_changed` で最新の view state 全体を受け取る。差分計算やマージは
//! せず、最後に受理した値がそのまま全購読者へ配られる。

use serde::{Deserialize, Serialize};

use crate::WorkspaceId;

/// Workspace の view state 共有を購読する。応答で現在の snapshot を返す。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceViewSubscribeRequest {
    pub workspace_id: WorkspaceId,
}

/// Workspace の view state 共有を購読解除する。以降この Session へは配信しない。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceViewUnsubscribeRequest {
    pub workspace_id: WorkspaceId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceViewUnsubscribeResponse {
    pub workspace_id: WorkspaceId,
}

/// 購読開始時点の view state。まだ誰も共有していない Workspace は `view` が `None`。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceViewSnapshot {
    pub revision: u64,
    pub view: Option<workbench::WorkspaceView>,
}

/// view state 全体を置き換える。Host は同時更新を直列化し last-writer-wins とする。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceViewUpdateRequest {
    pub workspace_id: WorkspaceId,
    pub view: workbench::WorkspaceView,
}

/// 受理した view state の revision。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceViewUpdateResponse {
    pub revision: u64,
}
