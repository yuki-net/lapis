//! Workspace切替中に送る Command の判定と、失敗時に元へ戻す attachment の順序。
//!
//! wasm とホスト側のテストの両方から使う。WebSocket の送信自体はここでは行わない。
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use std::path::Path;

use client_api::{
    RequestBody, ResponseBody, WorkspaceCloseRequest, WorkspaceConnectRequest,
    WorkspaceId as ApiWorkspaceId,
};
use editor_core::DocumentId;
use futures_channel::oneshot;
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
    SaveWorkbench {
        view: ConversationViewState,
    },
    RetryWorkbenchLocal,
    /// 自分のタブ構成とアクティブ文書を Workspace の view state として共有する。
    PublishView {
        view: workbench::WorkspaceView,
    },
    /// `workspace.view.subscribe` を取り直して共有状態を再同期する。
    ResyncView,
    /// 共有から抜ける。Workspace を閉じたときに送る。
    CloseView,
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
                | Self::SaveWorkbench { .. }
                | Self::RetryWorkbenchLocal
                | Self::PublishView { .. }
                | Self::CloseView
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

/// Web の仮想 root 以下のパスだけを、拡張へ渡す相対パスに変換する。
pub(crate) fn terminal_relative_cwd(path: &Path) -> Result<String, String> {
    let relative = path
        .strip_prefix(crate::web_replica::ROOT)
        .map_err(|_| "Terminal の作業パスが不正です")?;
    if relative.as_os_str().is_empty() {
        return Ok(".".into());
    }
    client_api::WorkspaceRelativePath::from_path(relative)
        .map(|path| path.as_str().to_owned())
        .map_err(|_| "Terminal の作業パスが不正です".into())
}

/// Host の同名・巨大・範囲外 session を描画器へ渡さない。
pub(crate) fn validate_terminal_sessions(
    sessions: &[crate::terminal_extension::v1::Session],
) -> Result<(), String> {
    let mut ids = std::collections::BTreeSet::new();
    if sessions.len() > 64
        || sessions.iter().any(|session| {
            session.id.is_empty()
                || session.id.len() > 256
                || session.columns == 0
                || session.columns > 512
                || session.rows == 0
                || session.rows > 256
                || !ids.insert(session.id.as_str())
                || (session.working_directory != "."
                    && client_api::WorkspaceRelativePath::from_path(Path::new(
                        &session.working_directory,
                    ))
                    .is_err())
        })
    {
        return Err("Terminal session 一覧が不正です".into());
    }
    Ok(())
}

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
    use crate::web_cache::{CachedWorkspace, WorkspaceCache};
    use crate::web_replica::Replica;
    use editor_core::WorkspaceId;

    #[test]
    fn terminal_paths_stay_in_web_workspace() {
        let root = Path::new(crate::web_replica::ROOT);
        assert_eq!(terminal_relative_cwd(root).unwrap(), ".");
        assert_eq!(terminal_relative_cwd(&root.join("src")).unwrap(), "src");
        assert!(terminal_relative_cwd(&root.join("..").join("outside")).is_err());
        assert!(terminal_relative_cwd(Path::new("/outside")).is_err());
    }

    #[test]
    fn terminal_session_listing_rejects_duplicates_and_unsafe_metadata() {
        use crate::terminal_extension::v1::Session;
        let valid = Session {
            id: "pty-1".into(),
            columns: 80,
            rows: 24,
            working_directory: ".".into(),
            owner: None,
        };
        assert!(validate_terminal_sessions(std::slice::from_ref(&valid)).is_ok());
        assert!(validate_terminal_sessions(&[valid.clone(), valid.clone()]).is_err());
        let mut invalid = valid;
        invalid.working_directory = "../outside".into();
        assert!(validate_terminal_sessions(&[invalid]).is_err());
    }

    fn id(value: &str) -> ApiWorkspaceId {
        ApiWorkspaceId::try_new(value.to_owned()).unwrap()
    }

    fn document() -> DocumentId {
        DocumentId::new("web-1")
    }

    fn cached(workspace: &str) -> CachedWorkspace {
        CachedWorkspace {
            replica: Replica::new(
                WorkspaceId::new(workspace),
                Vec::new(),
                None,
                Default::default(),
            ),
        }
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
            Command::PublishView {
                view: workbench::WorkspaceView {
                    open_documents: Vec::new(),
                    active_path: None,
                    active_document_id: None,
                },
            },
            Command::CloseView,
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
    fn view_resync_is_a_read() {
        // 共有状態の取り直しは Host を書き換えないため、切替中も止めない。
        assert!(!Command::ResyncView.is_mutation());
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

    #[test]
    fn failed_cached_resync_returns_the_cache_and_restores_the_attachment() {
        let current = id("current");
        let target = id("target");
        let workspace = WorkspaceId::new("target");
        let mut cache = WorkspaceCache::new(4);
        cache.insert(cached("target"));

        // 復帰時に取り出した複製の再同期が失敗した場合を想定する。
        let restored = cache.take(&workspace).expect("保持済み複製を取り出せる");
        // 設置せずに保持へ戻す。次の切替で同じ表示状態から再試行できる。
        assert!(cache.insert(restored).is_none());
        assert!(cache.take(&workspace).is_some());
        // 接続は切替先を閉じてから元の Workspace へ戻す。
        assert_eq!(
            rollback_steps(&current, Some(&target)),
            vec![
                AttachmentStep::Close(target),
                AttachmentStep::Connect(current),
            ]
        );
    }
}
