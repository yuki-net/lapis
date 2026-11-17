//! PTY の所有は Host に置き、ターミナル拡張はセッションの操作と状態を持つ。

#![allow(clippy::too_many_arguments)] // wit-bindgen の生成 ABI

wit_bindgen::generate!({ path: "wit", world: "terminal", generate_all });

use std::{
    cell::RefCell,
    collections::BTreeMap,
    time::{Duration, Instant},
};

use lapis::process_info::host as process_info;
use lapis::pty::sessions::{self, EnvironmentEntry, Size, SpawnRequest};
use serde_json::Value;
use terminal_session_api::v1::{
    self, Exit, IdRequest, ListRequest, ListResponse, ReadRequest, ReadResponse, ResizeRequest,
    Session as SessionInfo, StartRequest, StartResponse, WriteRequest,
};

struct Session {
    pty_id: String,
    attached: bool,
    columns: u16,
    rows: u16,
    working_directory: String,
    workspace: String,
    owner: Option<String>,
    last_activity: Instant,
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

fn owner(id: &str, workspace: &str) -> Result<Option<String>, String> {
    SESSIONS.with(|sessions| match sessions.borrow().get(id) {
        Some(session) if session.attached && session.workspace == workspace => {
            Ok(session.owner.clone())
        }
        _ => Err("terminal session is not attached".into()),
    })
}

fn pty_id(id: &str, workspace: &str) -> Result<String, String> {
    SESSIONS.with(|sessions| match sessions.borrow().get(id) {
        Some(session) if session.attached && session.workspace == workspace => {
            Ok(session.pty_id.clone())
        }
        _ => Err("terminal session is not attached".into()),
    })
}

fn require_owner(id: &str, workspace: &str, caller: &str) -> Result<(), String> {
    if owner(id, workspace)?.is_some_and(|owner| owner != caller) {
        return Err("terminal is owned by another session".into());
    }
    Ok(())
}

fn touch(id: &str) {
    SESSIONS.with(|sessions| {
        if let Some(session) = sessions.borrow_mut().get_mut(id) {
            session.last_activity = Instant::now();
        }
    });
}

fn expire_owners() {
    SESSIONS.with(|sessions| {
        for session in sessions.borrow_mut().values_mut() {
            if session.owner.is_some()
                && session.last_activity.elapsed() >= Duration::from_secs(300)
            {
                session.owner = None;
            }
        }
    });
}

fn reap_closed() {
    SESSIONS.with(|sessions| {
        sessions
            .borrow_mut()
            .retain(|_, session| session.attached || sessions::forget(&session.pty_id).is_err());
    });
}

fn run(operation: &str, payload: &str, caller: &str, workspace: &str) -> Result<Value, String> {
    reap_closed();
    expire_owners();
    match operation {
        "lapis.system.session-detached" | "lapis.system.release-claims" => {
            let _: serde_json::Value = decode(payload)?;
            SESSIONS.with(|sessions| {
                for session in sessions.borrow_mut().values_mut() {
                    if operation == "lapis.system.release-claims"
                        || session.owner.as_deref() == Some(caller)
                    {
                        session.owner = None;
                    }
                }
            });
            Ok(Value::Null)
        }
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
                    .filter(|(_, session)| session.attached && session.workspace == workspace)
                    .map(|(id, session)| SessionInfo {
                        id: id.clone(),
                        columns: session.columns,
                        rows: session.rows,
                        working_directory: session.working_directory.clone(),
                        owner: session.owner.clone(),
                    })
                    .collect::<Vec<_>>()
            });
            encode(ListResponse { sessions: active })
        }
        v1::START => {
            let input: StartRequest = decode(payload)?;
            if let Some(id) = &input.requested_id {
                let valid = id.strip_prefix("client-").is_some_and(|suffix| {
                    suffix.len() == 32
                        && suffix
                            .bytes()
                            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
                });
                if !valid || SESSIONS.with(|sessions| sessions.borrow().contains_key(id)) {
                    return Err("invalid or duplicate terminal session ID".into());
                }
            }
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
            let pty = sessions::spawn(&command)?;
            sessions::attach(&pty)?;
            let id = input.requested_id.unwrap_or_else(|| pty.clone());
            SESSIONS.with(|sessions| {
                sessions.borrow_mut().insert(
                    id.clone(),
                    Session {
                        pty_id: pty,
                        attached: true,
                        columns: input.columns,
                        rows: input.rows,
                        working_directory: input.working_directory,
                        workspace: workspace.to_owned(),
                        owner: Some(caller.to_owned()),
                        last_activity: Instant::now(),
                    },
                )
            });
            encode(StartResponse {
                id,
                owner: Some(caller.to_owned()),
            })
        }
        v1::WRITE => {
            let input: WriteRequest = decode(payload)?;
            require_owner(&input.id, workspace, caller)?;
            sessions::write(&pty_id(&input.id, workspace)?, &input.bytes)?;
            touch(&input.id);
            Ok(Value::Null)
        }
        v1::RESIZE => {
            let input: ResizeRequest = decode(payload)?;
            require_owner(&input.id, workspace, caller)?;
            sessions::resize(
                &pty_id(&input.id, workspace)?,
                Size {
                    columns: input.columns,
                    rows: input.rows,
                },
            )?;
            touch(&input.id);
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
            let current_owner = owner(&input.id, workspace)?;
            // JSON の byte 配列が dispatch 応答の上限を超えないよう制限する。
            if input.max_bytes == 0
                || input.max_bytes > v1::MAX_READ_BYTES
                || input.timeout_ms > v1::MAX_READ_WAIT_MS
            {
                return Err("invalid terminal read limit".into());
            }
            let output = sessions::read(
                &pty_id(&input.id, workspace)?,
                input.offset,
                input.max_bytes,
                input.timeout_ms,
            )?;
            encode(ReadResponse {
                bytes: output.bytes,
                next: output.next,
                lost_before: output.lost_before,
                exit: output.exit.map(|exit| Exit { code: exit.code }),
                read_error: output.read_error,
                write_error: output.write_error,
                owner: current_owner,
            })
        }
        v1::CLOSE => {
            let input: IdRequest = decode(payload)?;
            require_owner(&input.id, workspace, caller)?;
            let pty = pty_id(&input.id, workspace)?;
            sessions::kill(&pty)?;
            sessions::detach(&pty)?;
            SESSIONS.with(|sessions| {
                if let Some(session) = sessions.borrow_mut().get_mut(&input.id) {
                    session.attached = false;
                }
            });
            reap_closed();
            Ok(Value::Null)
        }
        v1::ATTACH => {
            let input: IdRequest = decode(payload)?;
            encode(v1::OwnerResponse {
                owner: owner(&input.id, workspace)?,
            })
        }
        v1::CLAIM | v1::RELEASE | v1::DETACH => {
            let input: IdRequest = decode(payload)?;
            let current = owner(&input.id, workspace)?;
            if operation == v1::RELEASE && current.as_deref().is_some_and(|owner| owner != caller) {
                return Err("terminal is owned by another session".into());
            }
            let next = if operation == v1::CLAIM {
                Some(caller.to_owned())
            } else if current.as_deref() == Some(caller) {
                None
            } else {
                current
            };
            SESSIONS.with(|sessions| {
                if let Some(session) = sessions.borrow_mut().get_mut(&input.id) {
                    session.owner = next.clone();
                    session.last_activity = Instant::now();
                }
            });
            encode(v1::OwnerResponse { owner: next })
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
                .map(|(_, session)| session.pty_id.clone())
                .collect::<Vec<_>>()
        });
        for id in ids {
            let _ = sessions::kill(&id);
            let _ = sessions::detach(&id);
        }
        SESSIONS.with(|sessions| sessions.borrow_mut().clear());
    }

    fn dispatch(
        operation: String,
        payload: String,
        session: String,
        workspace: String,
    ) -> Result<String, String> {
        if session.is_empty() || workspace.is_empty() {
            return Err("missing authenticated context".into());
        }
        serde_json::to_string(&run(&operation, &payload, &session, &workspace)?)
            .map_err(|error| error.to_string())
    }
}

export!(Terminal);
