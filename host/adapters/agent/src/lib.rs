//! 長時間実行する Task と外部 runner の境界。
//!
//! 外部 CLI 固有の JSON はこの crate で `TaskEvent` に変換し、UI へ公開しない。

use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::Receiver,
    },
    thread,
    time::Duration,
};

use serde_json::{Value, json};

pub use lapis_task::*;

#[derive(Clone, Debug)]
pub enum RunnerUpdate {
    Status(ExecutionStatus),
    Event(TaskEvent),
    ExternalThread(String),
    Failure(String),
}

#[derive(Clone, Debug)]
enum PendingRequest {
    Approval {
        id: Value,
    },
    Input {
        id: Value,
        question_ids: Vec<String>,
    },
}

/// Codex app-server の JSON-RPC を Lapis の共通イベントへ変換する。
///
/// この関数は worker プロセス内で同期実行する。UI プロセスから直接呼び出さない。
pub fn run_codex_app_server(
    spec: &CodexExecutionSpec,
    controls: Receiver<TaskControl>,
    mut update: impl FnMut(RunnerUpdate),
) -> Result<(), TaskError> {
    let mut child = Command::new(&spec.executable)
        .args(["app-server", "--stdio"])
        .current_dir(&spec.workspace_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            TaskError::new(format!(
                "Codex app-server を起動できませんでした ({}): {error}",
                spec.executable.display()
            ))
        })?;

    let stdin =
        Arc::new(Mutex::new(child.stdin.take().ok_or_else(|| {
            TaskError::new("Codex stdin を取得できませんでした")
        })?));
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| TaskError::new("Codex stdout を取得できませんでした"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| TaskError::new("Codex stderr を取得できませんでした"))?;

    let stderr_lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let stderr_target = stderr_lines.clone();
    thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if let Ok(mut lines) = stderr_target.lock() {
                if lines.len() >= 32 {
                    lines.remove(0);
                }
                lines.push(line);
            }
        }
    });

    let pending = Arc::new(Mutex::new(None::<PendingRequest>));
    let thread_id = Arc::new(Mutex::new(None::<String>));
    let turn_id = Arc::new(Mutex::new(None::<String>));
    let control_stdin = stdin.clone();
    let control_pending = pending.clone();
    let control_thread_id = thread_id.clone();
    let control_turn_id = turn_id.clone();
    let control_done = Arc::new(AtomicBool::new(false));
    let worker_done = control_done.clone();
    let cancel_requested = Arc::new(AtomicBool::new(false));
    let worker_cancel_requested = cancel_requested.clone();
    let control_thread = thread::spawn(move || {
        loop {
            if worker_done.load(Ordering::Relaxed) {
                break;
            }
            let control = match controls.recv_timeout(Duration::from_millis(100)) {
                Ok(control) => control,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            };
            let message = match control {
                TaskControl::Cancel => {
                    worker_cancel_requested.store(true, Ordering::Relaxed);
                    let thread_id = control_thread_id
                        .lock()
                        .ok()
                        .and_then(|value| value.clone());
                    let turn_id = control_turn_id.lock().ok().and_then(|value| value.clone());
                    match (thread_id, turn_id) {
                        (Some(thread_id), Some(turn_id)) => Some(json!({
                            "method": "turn/interrupt",
                            "id": 9001,
                            "params": { "threadId": thread_id, "turnId": turn_id }
                        })),
                        _ => None,
                    }
                }
                TaskControl::Approve | TaskControl::Decline | TaskControl::Reply { .. } => {
                    let request = control_pending
                        .lock()
                        .ok()
                        .and_then(|mut value| value.take());
                    match (control, request) {
                        (TaskControl::Approve, Some(PendingRequest::Approval { id })) => {
                            Some(json!({
                                "id": id,
                                "result": { "decision": "accept" }
                            }))
                        }
                        (TaskControl::Decline, Some(PendingRequest::Approval { id })) => {
                            Some(json!({
                                "id": id,
                                "result": { "decision": "decline" }
                            }))
                        }
                        (
                            TaskControl::Reply { text },
                            Some(PendingRequest::Input { id, question_ids }),
                        ) => {
                            let answers = question_ids
                                .into_iter()
                                .map(|question_id| (question_id, json!({ "answers": [text] })))
                                .collect::<serde_json::Map<_, _>>();
                            Some(json!({ "id": id, "result": { "answers": answers } }))
                        }
                        (_, request) => {
                            if let Some(request) = request
                                && let Ok(mut pending) = control_pending.lock()
                            {
                                *pending = Some(request);
                            }
                            None
                        }
                    }
                }
            };
            if let Some(message) = message {
                let _ = write_json_line(&control_stdin, &message);
            }
        }
    });

    write_json_line(
        &stdin,
        &json!({
            "method": "initialize",
            "id": 1,
            "params": {
                "clientInfo": { "name": "lapis", "title": "Lapis", "version": env!("CARGO_PKG_VERSION") },
                "capabilities": { "experimentalApi": true, "requestAttestation": false }
            }
        }),
    )?;

    let mut lines = BufReader::new(stdout).lines();
    wait_for_response(&mut lines, 1)?;
    write_json_line(&stdin, &json!({ "method": "initialized" }))?;
    write_json_line(
        &stdin,
        &json!({
            "method": "thread/start",
            "id": 2,
            "params": {
                "cwd": spec.workspace_root,
                "approvalPolicy": "on-request",
                "sandbox": "workspace-write",
                "ephemeral": false
            }
        }),
    )?;
    let thread_response = wait_for_response(&mut lines, 2)?;
    let external_thread = thread_response
        .pointer("/result/thread/id")
        .and_then(Value::as_str)
        .ok_or_else(|| TaskError::new("thread/start 応答に thread id がありません"))?
        .to_owned();
    if let Ok(mut value) = thread_id.lock() {
        *value = Some(external_thread.clone());
    }
    update(RunnerUpdate::ExternalThread(external_thread.clone()));
    let mut turn_start = json!({
        "method": "turn/start",
        "id": 3,
        "params": {
            "threadId": external_thread,
            "input": [{ "type": "text", "text": spec.prompt, "text_elements": [] }]
        }
    });
    if spec.mode == TaskMode::Plan {
        let model = thread_response
            .pointer("/result/model")
            .and_then(Value::as_str)
            .unwrap_or("gpt-5")
            .to_owned();
        turn_start["params"]["collaborationMode"] = json!({
            "mode": "plan",
            "settings": {
                "model": model,
                "reasoning_effort": null,
                "developer_instructions": null
            }
        });
    }
    write_json_line(&stdin, &turn_start)?;
    let turn_response = wait_for_response(&mut lines, 3)?;
    let external_turn = turn_response
        .pointer("/result/turn/id")
        .and_then(Value::as_str)
        .ok_or_else(|| TaskError::new("turn/start 応答に turn id がありません"))?
        .to_owned();
    if let Ok(mut value) = turn_id.lock() {
        *value = Some(external_turn);
    }
    update(RunnerUpdate::Status(ExecutionStatus::Running));

    let mut terminal = false;
    for line in lines {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = serde_json::from_str(&line)?;
        if cancel_requested.load(Ordering::Relaxed) {
            let _ = child.kill();
            update(RunnerUpdate::Status(ExecutionStatus::Cancelled));
            terminal = true;
            break;
        }
        if message.get("id").and_then(Value::as_i64) == Some(9001) {
            if let Some(error) = message.get("error") {
                update(RunnerUpdate::Event(TaskEvent::Error {
                    message: format!("取消要求が拒否されました: {error}"),
                }));
            } else {
                update(RunnerUpdate::Event(TaskEvent::Warning {
                    message: "取消要求を送信しました".to_owned(),
                }));
            }
            continue;
        }
        if let Some(method) = message.get("method").and_then(Value::as_str) {
            match method {
                "item/agentMessage/delta" => emit_delta(
                    &message,
                    "/params/delta",
                    |text| TaskEvent::MessageDelta { text },
                    &mut update,
                ),
                "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => emit_delta(
                    &message,
                    "/params/delta",
                    |text| TaskEvent::ReasoningDelta { text },
                    &mut update,
                ),
                "item/commandExecution/outputDelta" => emit_delta(
                    &message,
                    "/params/delta",
                    |text| TaskEvent::CommandOutput { text },
                    &mut update,
                ),
                "item/completed" => emit_completed_item(&message, &mut update),
                "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                    let id = message.get("id").cloned().unwrap_or(Value::Null);
                    let summary = message
                        .pointer("/params/command")
                        .or_else(|| message.pointer("/params/reason"))
                        .and_then(Value::as_str)
                        .unwrap_or("Codex が操作の許可を求めています")
                        .to_owned();
                    if let Ok(mut request) = pending.lock() {
                        *request = Some(PendingRequest::Approval { id: id.clone() });
                    }
                    update(RunnerUpdate::Status(ExecutionStatus::WaitingForApproval));
                    update(RunnerUpdate::Event(TaskEvent::ApprovalRequested {
                        request_id: id.to_string(),
                        summary,
                    }));
                }
                "item/tool/requestUserInput" => {
                    let id = message.get("id").cloned().unwrap_or(Value::Null);
                    let questions = message
                        .pointer("/params/questions")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    let question_ids = questions
                        .iter()
                        .filter_map(|question| question.get("id").and_then(Value::as_str))
                        .map(str::to_owned)
                        .collect::<Vec<_>>();
                    let prompt = questions
                        .iter()
                        .filter_map(|question| question.get("question").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join(" / ");
                    let secret = questions.iter().any(|question| {
                        question.get("isSecret").and_then(Value::as_bool) == Some(true)
                    });
                    if let Ok(mut request) = pending.lock() {
                        *request = Some(PendingRequest::Input {
                            id: id.clone(),
                            question_ids: question_ids.clone(),
                        });
                    }
                    update(RunnerUpdate::Status(ExecutionStatus::WaitingForInput));
                    update(RunnerUpdate::Event(TaskEvent::InputRequested {
                        request_id: id.to_string(),
                        question_ids,
                        prompt,
                        secret,
                    }));
                }
                "serverRequest/resolved" => {
                    if let Ok(mut request) = pending.lock() {
                        *request = None;
                    }
                    update(RunnerUpdate::Status(ExecutionStatus::Running));
                }
                "warning" | "deprecationNotice" | "configWarning" => {
                    let warning = message
                        .pointer("/params/message")
                        .and_then(Value::as_str)
                        .unwrap_or("Codex から警告が返されました")
                        .to_owned();
                    update(RunnerUpdate::Event(TaskEvent::Warning { message: warning }));
                }
                "error" => {
                    let error = extract_error(&message);
                    update(RunnerUpdate::Event(TaskEvent::Error {
                        message: error.clone(),
                    }));
                    if message
                        .pointer("/params/willRetry")
                        .and_then(Value::as_bool)
                        != Some(true)
                    {
                        update(RunnerUpdate::Failure(error));
                    }
                }
                "turn/completed" => {
                    let status = message
                        .pointer("/params/turn/status")
                        .and_then(Value::as_str)
                        .unwrap_or("failed");
                    let mapped = match status {
                        "completed" => ExecutionStatus::Succeeded,
                        "interrupted" => ExecutionStatus::Cancelled,
                        _ => ExecutionStatus::Failed,
                    };
                    if mapped == ExecutionStatus::Failed {
                        update(RunnerUpdate::Failure(extract_error(&message)));
                    }
                    update(RunnerUpdate::Status(mapped));
                    terminal = true;
                    break;
                }
                _ => {}
            }
        }
    }

    drop(stdin);
    control_done.store(true, Ordering::Relaxed);
    let _ = control_thread.join();
    let status = child.wait()?;
    if !terminal {
        let stderr = stderr_lines
            .lock()
            .map(|lines| lines.join("\n"))
            .unwrap_or_default();
        return Err(TaskError::new(format!(
            "Codex app-server が予期せず終了しました ({status}){}",
            if stderr.is_empty() {
                String::new()
            } else {
                format!(": {stderr}")
            }
        )));
    }
    Ok(())
}

fn write_json_line(stdin: &Arc<Mutex<impl Write>>, message: &Value) -> Result<(), TaskError> {
    let mut stdin = stdin
        .lock()
        .map_err(|_| TaskError::new("Codex stdin lock が破損しました"))?;
    serde_json::to_writer(&mut *stdin, message)?;
    stdin.write_all(b"\n")?;
    stdin.flush()?;
    Ok(())
}

fn wait_for_response(
    lines: &mut impl Iterator<Item = Result<String, std::io::Error>>,
    expected_id: i64,
) -> Result<Value, TaskError> {
    for line in lines {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = serde_json::from_str(&line)?;
        if message.get("id").and_then(Value::as_i64) == Some(expected_id) {
            if let Some(error) = message.get("error") {
                return Err(TaskError::new(format!("Codex JSON-RPC error: {error}")));
            }
            return Ok(message);
        }
    }
    Err(TaskError::new(format!(
        "Codex JSON-RPC response {expected_id} を受信できませんでした"
    )))
}

fn emit_delta(
    message: &Value,
    pointer: &str,
    event: impl FnOnce(String) -> TaskEvent,
    update: &mut impl FnMut(RunnerUpdate),
) {
    if let Some(text) = message.pointer(pointer).and_then(Value::as_str)
        && !text.is_empty()
    {
        update(RunnerUpdate::Event(event(text.to_owned())));
    }
}

fn emit_completed_item(message: &Value, update: &mut impl FnMut(RunnerUpdate)) {
    let Some(item) = message.pointer("/params/item") else {
        return;
    };
    match item.get("type").and_then(Value::as_str) {
        Some("commandExecution") => {
            let command = item
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("command")
                .to_owned();
            let success = item
                .get("exitCode")
                .and_then(Value::as_i64)
                .is_some_and(|code| code == 0);
            update(RunnerUpdate::Event(TaskEvent::CommandCompleted {
                command,
                success,
            }));
        }
        Some("fileChange") => {
            if let Some(path) = item
                .pointer("/changes/0/path")
                .or_else(|| item.get("path"))
                .and_then(Value::as_str)
            {
                update(RunnerUpdate::Event(TaskEvent::FileChanged {
                    path: path.to_owned(),
                }));
            }
        }
        _ => {}
    }
}

fn extract_error(message: &Value) -> String {
    message
        .pointer("/params/error/message")
        .or_else(|| message.pointer("/params/turn/error/message"))
        .or_else(|| message.pointer("/error/message"))
        .and_then(Value::as_str)
        .unwrap_or("Codex task failed")
        .to_owned()
}
