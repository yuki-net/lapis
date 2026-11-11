use serde::{Deserialize, Serialize};

use crate::{SessionId, TerminalId, WorkspaceId, WorkspaceRelativePath};

pub use ::terminal::{
    TerminalCell, TerminalColor, TerminalCursor, TerminalDelta, TerminalInput, TerminalKey,
    TerminalKeyEvent, TerminalModes, TerminalModifiers, TerminalProfile, TerminalProfileId,
    TerminalProfiles, TerminalRevision, TerminalRow, TerminalRowUpdate, TerminalScreen,
    TerminalScrollbackDelta, TerminalSize,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Terminal の実行状態。
pub enum TerminalStatus {
    Running,
    Exited,
    Terminated,
    Failed,
    Disconnected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Terminal の画面・状態のスナップショット。
pub struct TerminalSnapshot {
    pub terminal_id: TerminalId,
    pub workspace_id: WorkspaceId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<WorkspaceRelativePath>,
    pub status: TerminalStatus,
    pub revision: TerminalRevision,
    pub screen: TerminalScreen,
    /// 入力できる Session。`None` は誰も操作していない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<SessionId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Terminal を開始する。
pub struct TerminalStartRequest {
    pub workspace_id: WorkspaceId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<WorkspaceRelativePath>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Host が許可した profile。`None` は Host の既定 profile。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<TerminalProfileId>,
    pub size: TerminalSize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Host が起動を許可する Terminal profile を取得する。
pub struct TerminalListProfilesRequest {
    pub workspace_id: WorkspaceId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Host が許可する Terminal profile 一覧。
pub struct TerminalListProfilesResponse {
    pub profiles: TerminalProfiles,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// 開始した Terminal の初期状態。
pub struct TerminalStartResponse {
    pub terminal: TerminalSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Terminal へ入力・操作を送る。
pub struct TerminalInputRequest {
    pub terminal_id: TerminalId,
    pub input: TerminalInput,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Terminal の表示サイズを変更する。
pub struct TerminalResizeRequest {
    pub terminal_id: TerminalId,
    pub size: TerminalSize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Terminal を終了する。
pub struct TerminalTerminateRequest {
    pub terminal_id: TerminalId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Terminal の操作権を取得する。取得すると他の Session は入力できなくなる。
pub struct TerminalClaimRequest {
    pub terminal_id: TerminalId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
/// Terminal の操作権を手放す。
pub struct TerminalReleaseRequest {
    pub terminal_id: TerminalId,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
/// 入力・サイズ変更・終了を受理したことだけを返す。
///
/// 状態は応答に載せず、`EventBody::TerminalDelta` / `TerminalStatus` で通知する。
/// 取りこぼしは `SnapshotResync` で回復する。
pub struct TerminalCommandResponse {}
