//! ACP v1 の client 実装。
//!
//! 安定面の最小セットだけを扱う: `initialize` / `session/new` /
//! `session/prompt` / `session/update` / `session/request_permission` /
//! `session/cancel`。ACP 固有の型はこのモジュールの外へ出さず、
//! `AcpUpdate`（`task` の値）に変換して渡す。

use std::{
    fmt,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::Receiver,
    },
    thread,
    time::Duration,
};

use agent_client_protocol::{
    AcpAgent, AcpAgentConfig, Client, Dispatch, Responder,
    schema::ProtocolVersion,
    util::MatchDispatch,
    v1::{
        SessionMessage,
        schema::{
            CancelNotification, ClientCapabilities, ContentBlock, Implementation,
            InitializeRequest, PermissionOption, PermissionOptionKind, RequestPermissionOutcome,
            RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome,
            SessionNotification, StopReason, TextContent,
        },
    },
};
use task::{ExecutionStatus, TaskControl, TaskEvent};

use crate::map::session_update_event;

/// ACP agent を1回起動して 1 prompt を実行するための指定。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcpAgentSpec {
    /// ACP server を起動する実行ファイル。
    pub executable: PathBuf,
    /// 送信する prompt。
    pub prompt: String,
    /// `session/new` の cwd。agent のツール実行はこのディレクトリを基準にする。
    pub workspace_root: PathBuf,
}

/// ACP client から runner への通知。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AcpUpdate {
    Status(ExecutionStatus),
    Event(TaskEvent),
    /// agent が払い出した session ID。実行の同一性としてのみ扱う。
    ExternalThread(String),
    Failure(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcpError {
    message: String,
}

impl AcpError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for AcpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AcpError {}

impl From<agent_client_protocol::Error> for AcpError {
    fn from(error: agent_client_protocol::Error) -> Self {
        Self::new(error.to_string())
    }
}

/// 承認待ちの要求。応答は制御スレッドから行う。
struct PendingPermission {
    options: Vec<PermissionOption>,
    responder: Responder<RequestPermissionResponse>,
}

/// 実行中の接続へ cancel と承認を中継するスレッド。
struct ControlBridge {
    done: Arc<AtomicBool>,
    resumed: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl ControlBridge {
    fn start(
        connection: agent_client_protocol::ConnectionTo<agent_client_protocol::Agent>,
        session_id: agent_client_protocol::v1::schema::SessionId,
        controls: Receiver<TaskControl>,
        pending: Arc<Mutex<Option<PendingPermission>>>,
    ) -> Self {
        let done = Arc::new(AtomicBool::new(false));
        let resumed = Arc::new(AtomicBool::new(false));
        let control_done = done.clone();
        let control_resumed = resumed.clone();
        let handle = thread::spawn(move || {
            while !control_done.load(Ordering::Relaxed) {
                let control = match controls.recv_timeout(Duration::from_millis(100)) {
                    Ok(control) => control,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                };
                match control {
                    TaskControl::Cancel => {
                        // 承認待ちは cancel の結果として cancelled を返す。
                        let waiting = pending.lock().ok().and_then(|mut slot| slot.take());
                        if let Some(waiting) = waiting {
                            let _ = waiting.responder.respond(RequestPermissionResponse::new(
                                RequestPermissionOutcome::Cancelled,
                            ));
                        }
                        let _ = connection
                            .send_notification(CancelNotification::new(session_id.clone()));
                    }
                    TaskControl::Approve | TaskControl::Decline => {
                        let waiting = pending.lock().ok().and_then(|mut slot| slot.take());
                        if let Some(waiting) = waiting {
                            let allow = matches!(control, TaskControl::Approve);
                            let response = permission_response(&waiting.options, allow);
                            let _ = waiting.responder.respond(response);
                            control_resumed.store(true, Ordering::Relaxed);
                        }
                    }
                    // ACP の安定面に自由入力の受け口はない。
                    TaskControl::Reply { .. } => {}
                }
            }
        });
        Self {
            done,
            resumed,
            handle: Some(handle),
        }
    }

    /// 承認の応答を送ったかを取り出す。表示を実行中へ戻す判断に使う。
    fn take_resumed(&self) -> bool {
        self.resumed.swap(false, Ordering::Relaxed)
    }
}

impl Drop for ControlBridge {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// 承認要求へ返す応答。位置や名前ではなく種類で選び、常に狭い方を優先する。
fn permission_response(options: &[PermissionOption], allow: bool) -> RequestPermissionResponse {
    let preferred = if allow {
        [
            PermissionOptionKind::AllowOnce,
            PermissionOptionKind::AllowAlways,
        ]
    } else {
        [
            PermissionOptionKind::RejectOnce,
            PermissionOptionKind::RejectAlways,
        ]
    };
    let selected = preferred
        .iter()
        .find_map(|kind| options.iter().find(|option| option.kind == *kind));
    match selected {
        Some(option) => RequestPermissionResponse::new(RequestPermissionOutcome::Selected(
            SelectedPermissionOutcome::new(option.option_id.clone()),
        )),
        None => RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled),
    }
}

/// ACP agent を起動し、1 prompt の完了まで実行する。
///
/// `update` は worker スレッドで同期実行される。UI スレッドから直接呼ばない。
pub fn run_acp_session(
    spec: &AcpAgentSpec,
    controls: Receiver<TaskControl>,
    mut update: impl FnMut(AcpUpdate),
) -> Result<(), AcpError> {
    let agent = AcpAgent::new(AcpAgentConfig::new(&spec.executable).arg("acp"));
    futures::executor::block_on(run_connection(agent, spec, controls, &mut update))
}

async fn run_connection(
    agent: AcpAgent,
    spec: &AcpAgentSpec,
    controls: Receiver<TaskControl>,
    update: &mut impl FnMut(AcpUpdate),
) -> Result<(), AcpError> {
    let cwd = spec.workspace_root.clone();
    let prompt = spec.prompt.clone();
    let controls = Arc::new(Mutex::new(Some(controls)));
    let pending = Arc::new(Mutex::new(None::<PendingPermission>));
    let mut terminal = false;
    let mut failure: Option<String> = None;

    let result = Client
        .v1()
        .name("lapis")
        .connect_with(agent, async |cx| {
            cx.send_request(
                InitializeRequest::new(ProtocolVersion::V1)
                    .client_capabilities(ClientCapabilities::default())
                    .client_info(Implementation::new("lapis", env!("CARGO_PKG_VERSION"))),
            )
            .block_task()
            .await?;

            // cwd は ACP のツール実行基準になる。絶対パスで渡す。
            let setup = cx.build_session(&cwd).block_task().start_session().await?;
            let mut session = setup.session;
            let session_id = session.session_id().clone();
            update(AcpUpdate::ExternalThread(session_id.to_string()));

            let bridge = controls
                .lock()
                .ok()
                .and_then(|mut slot| slot.take())
                .map(|controls| {
                    ControlBridge::start(
                        session.connection().clone().into_raw(),
                        session_id,
                        controls,
                        pending.clone(),
                    )
                });

            update(AcpUpdate::Status(ExecutionStatus::Running));
            session
                .send_prompt_blocks(vec![ContentBlock::Text(TextContent::new(prompt.clone()))])?;

            loop {
                if bridge.as_ref().is_some_and(ControlBridge::take_resumed) {
                    update(AcpUpdate::Status(ExecutionStatus::Running));
                }
                match session.read_update().await? {
                    SessionMessage::Message(dispatch) => {
                        handle_dispatch(dispatch.into_raw(), &pending, update).await?;
                    }
                    SessionMessage::PromptResponse(response) => {
                        match response {
                            Ok(response) => {
                                let status = stop_status(&response.stop_reason);
                                if status == ExecutionStatus::Failed {
                                    failure = Some(format!(
                                        "ACP prompt が {:?} で終了しました",
                                        response.stop_reason
                                    ));
                                }
                                update(AcpUpdate::Status(status));
                            }
                            Err(error) => {
                                update(AcpUpdate::Failure(error.to_string()));
                                update(AcpUpdate::Status(ExecutionStatus::Failed));
                                return Err(error);
                            }
                        }
                        terminal = true;
                        break;
                    }
                    // 将来の session message は無視する。既知の経路だけを解釈する。
                    _ => {}
                }
            }
            drop(bridge);
            Ok(())
        })
        .await;

    if let Err(error) = result {
        if !terminal {
            update(AcpUpdate::Failure(error.to_string()));
            update(AcpUpdate::Status(ExecutionStatus::Failed));
        }
        return Err(AcpError::new(error.to_string()));
    }
    match failure {
        Some(failure) => Err(AcpError::new(failure)),
        None => Ok(()),
    }
}

/// agent からの 1 メッセージを処理する。未知の method は無視せず -32601 を返す。
async fn handle_dispatch(
    dispatch: Dispatch,
    pending: &Arc<Mutex<Option<PendingPermission>>>,
    update: &mut impl FnMut(AcpUpdate),
) -> Result<(), agent_client_protocol::Error> {
    let mut mappings: Vec<TaskEvent> = Vec::new();
    let mut approval: Option<(String, String)> = None;

    MatchDispatch::new(dispatch)
        .if_notification(async |notification: SessionNotification| {
            if let Some(event) = session_update_event(&notification.update) {
                mappings.push(event);
            }
            Ok(agent_client_protocol::Handled::Yes)
        })
        .await
        .if_request(async |request: RequestPermissionRequest, responder| {
            let summary = request
                .tool_call
                .fields
                .title
                .clone()
                .or_else(|| request.tool_call.fields.name.clone())
                .unwrap_or_else(|| "agent が操作の許可を求めています".to_owned());
            let options = request.options.clone();
            let request_id = request.tool_call.tool_call_id.to_string();
            if let Ok(mut slot) = pending.lock() {
                *slot = Some(PendingPermission { options, responder });
            }
            approval = Some((request_id, summary));
            Ok(agent_client_protocol::Handled::Yes)
        })
        .await
        .otherwise_ignore()?;

    for event in mappings {
        update(AcpUpdate::Event(event));
    }
    if let Some((request_id, summary)) = approval {
        update(AcpUpdate::Status(ExecutionStatus::WaitingForApproval));
        update(AcpUpdate::Event(TaskEvent::ApprovalRequested {
            request_id,
            summary,
        }));
    }
    Ok(())
}

/// prompt の終了理由を実行状態へ写す。
fn stop_status(reason: &StopReason) -> ExecutionStatus {
    match reason {
        StopReason::EndTurn => ExecutionStatus::Succeeded,
        StopReason::Cancelled => ExecutionStatus::Cancelled,
        _ => ExecutionStatus::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_reasons_map_to_execution_status() {
        assert_eq!(
            stop_status(&StopReason::EndTurn),
            ExecutionStatus::Succeeded
        );
        assert_eq!(
            stop_status(&StopReason::Cancelled),
            ExecutionStatus::Cancelled
        );
        assert_eq!(stop_status(&StopReason::MaxTokens), ExecutionStatus::Failed);
    }

    #[test]
    fn approval_prefers_the_narrowest_matching_option() {
        let options = vec![
            PermissionOption::new("always", "Always", PermissionOptionKind::AllowAlways),
            PermissionOption::new("once", "Once", PermissionOptionKind::AllowOnce),
            PermissionOption::new("reject", "Reject", PermissionOptionKind::RejectOnce),
        ];
        let response = permission_response(&options, true);
        assert_eq!(
            response.outcome,
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new("once"))
        );
    }

    #[test]
    fn rejection_uses_a_reject_option() {
        let options = vec![
            PermissionOption::new("once", "Once", PermissionOptionKind::AllowOnce),
            PermissionOption::new("never", "Never", PermissionOptionKind::RejectAlways),
        ];
        let response = permission_response(&options, false);
        assert_eq!(
            response.outcome,
            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new("never"))
        );
    }

    #[test]
    fn missing_option_cancels_instead_of_guessing() {
        let options = vec![PermissionOption::new(
            "once",
            "Once",
            PermissionOptionKind::AllowOnce,
        )];
        assert_eq!(
            permission_response(&options, false).outcome,
            RequestPermissionOutcome::Cancelled
        );
    }
}
