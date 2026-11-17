//! PTY の所有は Host に置き、ターミナル拡張はセッションの操作と状態を持つ。

wit_bindgen::generate!({ path: "wit", world: "terminal", generate_all });

use std::{cell::RefCell, collections::BTreeMap};

use lapis::pty::sessions::{self, EnvironmentEntry, Size, SpawnRequest};
use serde::Deserialize;
use serde_json::{Value, json};

// false は停止済み・detach 済みで、Host の後片付け待ち。
thread_local! {
    static SESSIONS: RefCell<BTreeMap<String, bool>> = const { RefCell::new(BTreeMap::new()) };
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    program: String,
    #[serde(default)]
    arguments: Vec<String>,
    #[serde(default)]
    environment: BTreeMap<String, String>,
    working_directory: String,
    columns: u16,
    rows: u16,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Id {
    id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    id: String,
    bytes: Vec<u8>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Resize {
    id: String,
    columns: u16,
    rows: u16,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Read {
    id: String,
    offset: u64,
    max_bytes: u32,
    timeout_ms: u32,
}

fn decode<T: serde::de::DeserializeOwned>(payload: &str) -> Result<T, String> {
    serde_json::from_str(payload).map_err(|_| "invalid terminal request".to_owned())
}

fn attached(id: &str) -> Result<(), String> {
    SESSIONS.with(|sessions| match sessions.borrow().get(id) {
        Some(true) => Ok(()),
        _ => Err("terminal session is not attached".into()),
    })
}

fn reap_closed() {
    SESSIONS.with(|sessions| {
        sessions
            .borrow_mut()
            .retain(|id, attached| *attached || sessions::forget(id).is_err());
    });
}

fn run(operation: &str, payload: &str) -> Result<Value, String> {
    reap_closed();
    match operation {
        "start" => {
            let input: Start = decode(payload)?;
            if input.columns == 0 || input.rows == 0 {
                return Err("invalid terminal size".into());
            }
            let command = SpawnRequest {
                program: input.program,
                arguments: input.arguments,
                environment: input
                    .environment
                    .into_iter()
                    .map(|(name, value)| EnvironmentEntry { name, value })
                    .collect(),
                working_directory: input.working_directory,
                initial_size: Size {
                    columns: input.columns,
                    rows: input.rows,
                },
            };
            let id = sessions::spawn(&command)?;
            sessions::attach(&id)?;
            SESSIONS.with(|sessions| sessions.borrow_mut().insert(id.clone(), true));
            Ok(json!({ "id": id }))
        }
        "write" => {
            let input: Input = decode(payload)?;
            attached(&input.id)?;
            sessions::write(&input.id, &input.bytes)?;
            Ok(Value::Null)
        }
        "resize" => {
            let input: Resize = decode(payload)?;
            attached(&input.id)?;
            sessions::resize(
                &input.id,
                Size {
                    columns: input.columns,
                    rows: input.rows,
                },
            )?;
            Ok(Value::Null)
        }
        "read" => {
            let input: Read = decode(payload)?;
            attached(&input.id)?;
            // JSON の byte 配列が dispatch 応答の上限を超えないよう制限する。
            if input.max_bytes == 0 || input.max_bytes > 8 * 1024 || input.timeout_ms > 1_000 {
                return Err("invalid terminal read limit".into());
            }
            let output =
                sessions::read(&input.id, input.offset, input.max_bytes, input.timeout_ms)?;
            Ok(json!({
                "bytes": output.bytes, "next": output.next, "lost_before": output.lost_before,
                "exit": output.exit.map(|exit| json!({ "code": exit.code })),
                "read_error": output.read_error, "write_error": output.write_error,
            }))
        }
        "close" => {
            let input: Id = decode(payload)?;
            attached(&input.id)?;
            sessions::kill(&input.id)?;
            sessions::detach(&input.id)?;
            SESSIONS.with(|sessions| sessions.borrow_mut().insert(input.id, false));
            reap_closed();
            Ok(Value::Null)
        }
        _ => Err("unknown terminal operation".into()),
    }
}

struct Terminal;

impl Guest for Terminal {
    fn activate() {}

    fn deactivate() {
        let ids = SESSIONS.with(|sessions| {
            sessions
                .borrow()
                .iter()
                .filter(|(_, attached)| **attached)
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>()
        });
        for id in ids {
            let _ = sessions::kill(&id);
            let _ = sessions::detach(&id);
        }
        SESSIONS.with(|sessions| sessions.borrow_mut().clear());
    }

    fn dispatch(operation: String, payload: String) -> Result<String, String> {
        serde_json::to_string(&run(&operation, &payload)?).map_err(|error| error.to_string())
    }
}

export!(Terminal);
