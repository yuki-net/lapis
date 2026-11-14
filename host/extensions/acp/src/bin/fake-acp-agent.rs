//! テスト用の最小 ACP agent。実ハーネスを CI に要求せずに client 側を検証する。
//!
//! stdio の改行区切り JSON-RPC だけを扱う。`initialize` / `session/new` /
//! `session/prompt` に応答し、`session/update` を通知する。
//!
//! prompt の文字列で挙動を切り替える。
//! - `approvals` を含む場合: 承認要求を 2 件送り、両方への応答が届くまで
//!   何も送らない。揃ってから完了を返す（再表示の起床を検証するため）
//! - それ以外: 通常の応答を返す

use std::io::{BufRead, Write};

use serde_json::{Value, json};

fn main() -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    // 承認待ちの prompt 要求 ID と、応答済みの承認 ID。
    let mut pending_prompt: Option<Value> = None;
    let mut approvals: Vec<String> = Vec::new();

    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(&line) {
            Ok(message) => message,
            Err(_) => continue,
        };

        let Some(method) = message.get("method").and_then(Value::as_str) else {
            // client からの応答。承認が 2 件揃ったら prompt を完了させる。
            if let Some(id) = message.get("id").and_then(Value::as_str)
                && id.starts_with("call-")
                && !approvals.iter().any(|value| value == id)
            {
                approvals.push(id.to_owned());
            }
            if approvals.len() >= 2
                && let Some(prompt_id) = pending_prompt.take()
            {
                notify(
                    &mut stdout,
                    json!({
                        "sessionId": "ses_fake",
                        "update": {
                            "sessionUpdate": "agent_message_chunk",
                            "content": { "type": "text", "text": "approved" }
                        }
                    }),
                )?;
                respond(
                    &mut stdout,
                    Some(prompt_id),
                    json!({ "stopReason": "end_turn" }),
                )?;
            }
            continue;
        };

        let id = message.get("id").cloned();
        match method {
            "initialize" => respond(
                &mut stdout,
                id,
                json!({
                    "protocolVersion": 1,
                    "agentCapabilities": {},
                    "authMethods": [],
                    "agentInfo": { "name": "fake-acp-agent", "version": "0.0.0" }
                }),
            )?,
            "session/new" => respond(&mut stdout, id, json!({ "sessionId": "ses_fake" }))?,
            "session/prompt" => {
                let session_id = message
                    .pointer("/params/sessionId")
                    .cloned()
                    .unwrap_or_else(|| json!("ses_fake"));
                let prompt = message
                    .pointer("/params/prompt/0/text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();

                if prompt.contains("approvals") {
                    // 承認要求 A と B を送る。応答が揃うまで何も送らない。
                    approvals.clear();
                    pending_prompt = id;
                    request_permission(&mut stdout, "call-a", "approval A")?;
                    request_permission(&mut stdout, "call-b", "approval B")?;
                    continue;
                }

                notify(
                    &mut stdout,
                    json!({
                        "sessionId": session_id,
                        "update": {
                            "sessionUpdate": "agent_message_chunk",
                            "content": { "type": "text", "text": "pong" }
                        }
                    }),
                )?;
                notify(
                    &mut stdout,
                    json!({
                        "sessionId": session_id,
                        "update": {
                            "sessionUpdate": "tool_call",
                            "toolCallId": "call_1",
                            "title": "read README",
                            "kind": "read",
                            "status": "pending"
                        }
                    }),
                )?;
                respond(&mut stdout, id, json!({ "stopReason": "end_turn" }))?;
            }
            "session/cancel" => {
                // キャンセルは prompt 応答側で表現される。ここでは何もしない。
            }
            _ => {
                if let Some(id) = id {
                    let response = json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32601, "message": "method not found" }
                    });
                    writeln!(stdout, "{response}")?;
                    stdout.flush()?;
                }
            }
        }
    }
    Ok(())
}

fn request_permission(
    stdout: &mut std::io::Stdout,
    call_id: &str,
    title: &str,
) -> std::io::Result<()> {
    let request = json!({
        "jsonrpc": "2.0",
        "id": call_id,
        "method": "session/request_permission",
        "params": {
            "sessionId": "ses_fake",
            "toolCall": {
                "toolCallId": call_id,
                "title": title,
                "kind": "edit",
                "status": "pending"
            },
            "options": [
                { "optionId": "allow-once", "name": "Allow once", "kind": "allow_once" },
                { "optionId": "reject-once", "name": "Reject", "kind": "reject_once" }
            ]
        }
    });
    writeln!(stdout, "{request}")?;
    stdout.flush()
}

fn respond(stdout: &mut std::io::Stdout, id: Option<Value>, result: Value) -> std::io::Result<()> {
    let Some(id) = id else {
        return Ok(());
    };
    let response = json!({ "jsonrpc": "2.0", "id": id, "result": result });
    writeln!(stdout, "{response}")?;
    stdout.flush()
}

fn notify(stdout: &mut std::io::Stdout, params: Value) -> std::io::Result<()> {
    let notification = json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": params
    });
    writeln!(stdout, "{notification}")?;
    stdout.flush()
}
