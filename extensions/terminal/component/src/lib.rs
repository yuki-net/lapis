//! PTY の所有は Host に置き、ターミナル拡張はセッションの操作と状態を持つ。

wit_bindgen::generate!({ path: "wit", world: "terminal", generate_all });

use std::{cell::RefCell, collections::BTreeMap};

use lapis::process_info::host as process_info;
use lapis::pty::sessions::{self, EnvironmentEntry, Size, SpawnRequest};
use serde_json::Value;
use terminal_session_api::v1::{
    self, Exit, IdRequest, ListRequest, ListResponse, ReadRequest, ReadResponse, ResizeRequest,
    Session as SessionInfo, StartRequest, StartResponse, WriteRequest,
};

struct Session {
    attached: bool,
    columns: u16,
    rows: u16,
    working_directory: String,
}

// attached が false のものは停止・detach 済みで Host の後片付け待ち。
thread_local! {
    static SESSIONS: RefCell<BTreeMap<String, Session>> = const { RefCell::new(BTreeMap::new()) };
}

fn decode<T: serde::de::DeserializeOwned>(payload: &str) -> Result<T, String> {
    serde_json::from_str(payload).map_err(|_| "invalid terminal request".to_owned())
}

fn check_directory_budget(directory: &str) -> Result<(), String> {
    let encoded = serde_json::to_string(directory)
        .map_err(|error| error.to_string())?
        .len();
    let existing = SESSIONS.with(|sessions| {
        sessions.borrow().values().try_fold(0usize, |sum, session| {
            serde_json::to_string(&session.working_directory)
                .ok()
                .and_then(|value| sum.checked_add(value.len()))
        })
    });
    if directory.is_empty()
        || encoded > 8 * 1024
        || existing
            .and_then(|value| value.checked_add(encoded))
            .is_none_or(|value| value > 32 * 1024)
    {
        return Err("terminal working directory limit exceeded".into());
    }
    Ok(())
}

fn encode<T: serde::Serialize>(value: T) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|error| error.to_string())
}

fn attached(id: &str) -> Result<(), String> {
    SESSIONS.with(|sessions| match sessions.borrow().get(id) {
        Some(session) if session.attached => Ok(()),
        _ => Err("terminal session is not attached".into()),
    })
}

fn reap_closed() {
    SESSIONS.with(|sessions| {
        sessions
            .borrow_mut()
            .retain(|id, session| session.attached || sessions::forget(id).is_err());
    });
}

fn run(operation: &str, payload: &str) -> Result<Value, String> {
    reap_closed();
    match operation {
        v1::INFO => {
            let _: v1::InfoRequest = decode(payload)?;
            let info = process_info::get()?;
            let pty = match info.pty {
                process_info::PtyFlavor::Conpty => v1::PtyFlavor::Conpty,
                process_info::PtyFlavor::Posix => v1::PtyFlavor::Posix,
            };
            encode(v1::InfoResponse {
                pty,
                default_shell: v1::ShellCommand {
                    program: info.default_shell.program,
                    arguments: info.default_shell.arguments,
                    environment: Default::default(),
                },
            })
        }
        v1::LIST => {
            let _: ListRequest = decode(payload)?;
            let active = SESSIONS.with(|sessions| {
                sessions
                    .borrow()
                    .iter()
                    .filter(|(_, session)| session.attached)
                    .map(|(id, session)| SessionInfo {
                        id: id.clone(),
                        columns: session.columns,
                        rows: session.rows,
                        working_directory: session.working_directory.clone(),
                    })
                    .collect::<Vec<_>>()
            });
            encode(ListResponse { sessions: active })
        }
        v1::START => {
            let input: StartRequest = decode(payload)?;
            check_directory_budget(&input.working_directory)?;
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
                working_directory: input.working_directory.clone(),
                initial_size: Size {
                    columns: input.columns,
                    rows: input.rows,
                },
            };
            let id = sessions::spawn(&command)?;
            sessions::attach(&id)?;
            SESSIONS.with(|sessions| {
                sessions.borrow_mut().insert(
                    id.clone(),
                    Session {
                        attached: true,
                        columns: input.columns,
                        rows: input.rows,
                        working_directory: input.working_directory,
                    },
                )
            });
            encode(StartResponse { id })
        }
        v1::WRITE => {
            let input: WriteRequest = decode(payload)?;
            attached(&input.id)?;
            sessions::write(&input.id, &input.bytes)?;
            Ok(Value::Null)
        }
        v1::RESIZE => {
            let input: ResizeRequest = decode(payload)?;
            attached(&input.id)?;
            sessions::resize(
                &input.id,
                Size {
                    columns: input.columns,
                    rows: input.rows,
                },
            )?;
            SESSIONS.with(|sessions| {
                if let Some(session) = sessions.borrow_mut().get_mut(&input.id) {
                    session.columns = input.columns;
                    session.rows = input.rows;
                }
            });
            Ok(Value::Null)
        }
        v1::READ => {
            let input: ReadRequest = decode(payload)?;
            attached(&input.id)?;
            // JSON の byte 配列が dispatch 応答の上限を超えないよう制限する。
            if input.max_bytes == 0
                || input.max_bytes > v1::MAX_READ_BYTES
                || input.timeout_ms > v1::MAX_READ_WAIT_MS
            {
                return Err("invalid terminal read limit".into());
            }
            let output =
                sessions::read(&input.id, input.offset, input.max_bytes, input.timeout_ms)?;
            encode(ReadResponse {
                bytes: output.bytes,
                next: output.next,
                lost_before: output.lost_before,
                exit: output.exit.map(|exit| Exit { code: exit.code }),
                read_error: output.read_error,
                write_error: output.write_error,
            })
        }
        v1::CLOSE => {
            let input: IdRequest = decode(payload)?;
            attached(&input.id)?;
            sessions::kill(&input.id)?;
            sessions::detach(&input.id)?;
            SESSIONS.with(|sessions| {
                if let Some(session) = sessions.borrow_mut().get_mut(&input.id) {
                    session.attached = false;
                }
            });
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
                .filter(|(_, session)| session.attached)
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
