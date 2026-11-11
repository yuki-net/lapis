//! Workspace切替中に送る Command の判定と、失敗時に元へ戻す attachment の順序。
//!
//! wasm とホスト側のテストの両方から使う。WebSocket の送信自体はここでは行わない。
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use std::path::PathBuf;

use client_api::{
    RequestBody, ResponseBody, WorkspaceCloseRequest, WorkspaceConnectRequest,
    WorkspaceId as ApiWorkspaceId,
};
use editor_core::DocumentId;
use futures_channel::oneshot;
use terminal::{TerminalId, TerminalInput};
use workbench::ConversationViewState;

/// 送信待ちのCommand。
/// Workbench保存のCommandだけが大きいが、送信待ちは短命なのでBox化はしない。
#[allow(clippy::large_enum_variant)]
pub(crate) enum Command {
    Open {
        local: DocumentId,
    },
    Sync {
        local: DocumentId,
    },
    Save {
        local: DocumentId,
    },
    Claim {
        local: DocumentId,
    },
    Release {
        local: DocumentId,
    },
    Reload {
        local: DocumentId,
    },
    Discard {
        local: DocumentId,
    },
    Close {
        local: DocumentId,
    },
    StartTerminal {
        local: TerminalId,
        cwd: Option<PathBuf>,
        columns: u16,
        rows: u16,
        profile: Option<terminal::TerminalProfileId>,
    },
    TerminalInput {
        local: TerminalId,
        input: TerminalInput,
    },
    TerminalResize {
        local: TerminalId,
        columns: u16,
        rows: u16,
    },
    TerminalTerminate {
        local: TerminalId,
    },
    TerminalClaim {
        local: TerminalId,
    },
    TerminalRelease {
        local: TerminalId,
    },
    SaveWorkbench {
        view: ConversationViewState,
    },
    RetryWorkbenchLocal,
    /// ここまでに積まれたCommandの処理完了を待つ。
    Barrier {
        done: oneshot::Sender<()>,
    },
}

impl Command {
    /// 画面の読み取りではなく、未保存の変更や Host 側の状態を書き換えるか。
    pub(crate) fn is_mutation(&self) -> bool {
        matches!(
            self,
            Self::Sync { .. }
                | Self::Save { .. }
                | Self::Claim { .. }
                | Self::Release { .. }
                | Self::Reload { .. }
                | Self::Discard { .. }
                | Self::Close { .. }
                | Self::StartTerminal { .. }
                | Self::TerminalInput { .. }
                | Self::TerminalResize { .. }
                | Self::TerminalTerminate { .. }
                | Self::TerminalClaim { .. }
                | Self::TerminalRelease { .. }
                | Self::SaveWorkbench { .. }
                | Self::RetryWorkbenchLocal
        )
    }
}

/// 切替中に受け付ける操作の種別。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Operation {
    /// 表示や読み取りだけを行う。
    #[allow(dead_code)]
    Read,
    /// 未保存の変更を生む、または Host の状態を書き換える。
    Mutation,
}

pub(crate) const SWITCHING_MESSAGE: &str = "Workspace切替中です";

/// 切替中は変更系だけを拒否する。読み取り系は従来どおり通す。
pub(crate) fn ensure_allowed(switching: bool, operation: Operation) -> Result<(), &'static str> {
    match (switching, operation) {
        (true, Operation::Mutation) => Err(SWITCHING_MESSAGE),
        _ => Ok(()),
    }
}

/// 切替失敗時に元へ戻す attachment の操作。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AttachmentStep {
    /// 接続済みの切替先を閉じる。
    Close(ApiWorkspaceId),
    /// 元の Workspace へ接続し直す。
    Connect(ApiWorkspaceId),
}

/// 元へ戻す順序。接続に成功した切替先がある場合だけ、先に閉じる。
pub(crate) fn rollback_steps(
    current: &ApiWorkspaceId,
    attempted: Option<&ApiWorkspaceId>,
) -> Vec<AttachmentStep> {
    let mut steps = Vec::with_capacity(2);
    if let Some(attempted) = attempted {
        steps.push(AttachmentStep::Close(attempted.clone()));
    }
    steps.push(AttachmentStep::Connect(current.clone()));
    steps
}

/// 手順に対応する要求。Close と Connect 以外は返さない。
pub(crate) fn attachment_request(step: &AttachmentStep) -> RequestBody {
    match step {
        AttachmentStep::Close(workspace_id) => RequestBody::WorkspaceClose(WorkspaceCloseRequest {
            workspace_id: workspace_id.clone(),
        }),
        AttachmentStep::Connect(workspace_id) => {
            RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
                workspace_id: workspace_id.clone(),
            })
        }
    }
}

/// `workspace.connect` の応答を検証する。接続以外の応答は失敗として扱う。
pub(crate) fn ensure_connected(body: ResponseBody) -> Result<(), String> {
    match body {
        ResponseBody::WorkspaceConnect(_) => Ok(()),
        ResponseBody::Error(error) => Err(error.code.as_str().to_owned()),
        _ => Err("Workspaceへ再接続できません".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: &str) -> ApiWorkspaceId {
        ApiWorkspaceId::try_new(value.to_owned()).unwrap()
    }

    fn document() -> DocumentId {
        DocumentId::new("web-1")
    }

    #[test]
    fn document_edit_commands_are_mutations() {
        let commands = [
            Command::Sync { local: document() },
            Command::Save { local: document() },
            Command::Claim { local: document() },
            Command::Release { local: document() },
            Command::Reload { local: document() },
            Command::Discard { local: document() },
            Command::Close { local: document() },
            Command::RetryWorkbenchLocal,
        ];
        for command in commands {
            assert!(command.is_mutation(), "変更系Commandを読み取り扱いしない");
        }
    }

    #[test]
    fn open_is_still_a_read() {
        assert!(!Command::Open { local: document() }.is_mutation());
    }

    #[test]
    fn switching_rejects_only_mutations() {
        assert_eq!(
            ensure_allowed(true, Operation::Mutation),
            Err(SWITCHING_MESSAGE)
        );
        assert_eq!(ensure_allowed(true, Operation::Read), Ok(()));
        assert_eq!(ensure_allowed(false, Operation::Mutation), Ok(()));
    }

    #[test]
    fn rollback_closes_the_attempted_workspace_before_reconnecting() {
        let current = id("current");
        let attempted = id("attempted");
        assert_eq!(
            rollback_steps(&current, Some(&attempted)),
            vec![
                AttachmentStep::Close(attempted),
                AttachmentStep::Connect(current),
            ]
        );
    }

    #[test]
    fn connect_failure_rolls_back_without_closing_the_attempted_workspace() {
        let current = id("current");
        assert_eq!(
            rollback_steps(&current, None),
            vec![AttachmentStep::Connect(current)]
        );
    }
}
