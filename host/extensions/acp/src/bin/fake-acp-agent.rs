//! テスト用の最小 ACP agent。実ハーネスを CI に要求せずに client 側を検証する。
//!
//! stdio の改行区切り JSON-RPC だけを扱う。`initialize` / `session/new` /
//! `session/prompt` / `session/cancel` に応答し、`session/update` を通知する。

use std::io::{BufRead, Write};

use serde_json::{Value, json};

fn main() -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
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
            // client からの応答はこの agent では使わない。
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
