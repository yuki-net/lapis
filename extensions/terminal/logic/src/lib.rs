//! 実行方式に依存しない Terminal セッションの判断。

use std::{
    cell::RefCell,
    collections::BTreeMap,
    time::{Duration, Instant},
};

use serde_json::Value;
use sessions::{EnvironmentEntry, Size, SpawnRequest};
use terminal_session_api::v1::{
    self, Exit, IdRequest, ListRequest, ListResponse, ReadRequest, ReadResponse, ResizeRequest,
    Session as SessionInfo, StartRequest, StartResponse, WriteRequest,
};

/// 実行アダプターが公開された Host 能力へ接続する。
pub trait TerminalHost {
    fn information(&self) -> Result<process_info::Information, String>;
    fn spawn(&self, command: &sessions::SpawnRequest) -> Result<String, String>;
    fn attach(&self, id: &str) -> Result<(), String>;
    fn detach(&self, id: &str) -> Result<(), String>;
    fn write(&self, id: &str, bytes: &[u8]) -> Result<(), String>;
    fn resize(&self, id: &str, size: sessions::Size) -> Result<(), String>;
    fn read(
        &self,
        id: &str,
        offset: u64,
        max_bytes: u32,
        timeout_ms: u32,
    ) -> Result<sessions::Output, String>;
    fn kill(&self, id: &str) -> Result<(), String>;
    fn forget(&self, id: &str) -> Result<(), String>;
}

thread_local! {
    static HOST: RefCell<Option<Box<dyn TerminalHost>>> = RefCell::new(None);
}

fn host<T>(f: impl FnOnce(&dyn TerminalHost) -> Result<T, String>) -> Result<T, String> {
    HOST.with(|slot| {
        f(slot
            .borrow()
            .as_deref()
            .ok_or("terminal host is not active")?)
    })
}

pub mod process_info {
    #[derive(Clone, Copy)]
    pub enum PtyFlavor {
        Conpty,
        Posix,
    }
    pub struct Command {
        pub program: String,
        pub arguments: Vec<String>,
    }
    pub struct Information {
        pub pty: PtyFlavor,
        pub default_shell: Command,
    }
    pub fn get() -> Result<Information, String> {
        super::host(|host| host.information())
    }
}

pub mod sessions {
    #[derive(Clone, Copy)]
    pub struct Size {
        pub columns: u16,
        pub rows: u16,
    }
    pub struct EnvironmentEntry {
        pub name: String,
        pub value: String,
    }
    pub struct SpawnRequest {
        pub program: String,
        pub arguments: Vec<String>,
        pub environment: Vec<EnvironmentEntry>,
        pub working_directory: String,
        pub initial_size: Size,
    }
    pub struct ExitStatus {
        pub code: Option<i32>,
    }
    pub struct Output {
        pub bytes: Vec<u8>,
        pub next: u64,
        pub lost_before: Option<u64>,
        pub exit: Option<ExitStatus>,
        pub read_error: Option<String>,
        pub write_error: Option<String>,
    }
    pub fn spawn(command: &SpawnRequest) -> Result<String, String> {
        super::host(|host| host.spawn(command))
    }
    pub fn attach(id: &str) -> Result<(), String> {
        super::host(|host| host.attach(id))
    }
    pub fn detach(id: &str) -> Result<(), String> {
        super::host(|host| host.detach(id))
    }
    pub fn write(id: &str, bytes: &[u8]) -> Result<(), String> {
        super::host(|host| host.write(id, bytes))
    }
    pub fn resize(id: &str, size: Size) -> Result<(), String> {
        super::host(|host| host.resize(id, size))
    }
    pub fn read(id: &str, offset: u64, max_bytes: u32, timeout_ms: u32) -> Result<Output, String> {
        super::host(|host| host.read(id, offset, max_bytes, timeout_ms))
    }
    pub fn kill(id: &str) -> Result<(), String> {
        super::host(|host| host.kill(id))
    }
    pub fn forget(id: &str) -> Result<(), String> {
        super::host(|host| host.forget(id))
    }
}

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

/// Profile は Terminal 拡張が所有し、Host は既定の実行環境だけを提供する。
fn profiles() -> Result<v1::ProfilesResponse, String> {
    let info = process_info::get()?;
    let default_command = v1::ShellCommand {
        program: info.default_shell.program,
        arguments: info.default_shell.arguments,
        environment: Default::default(),
    };
    let make = |id: &str, name: &str, program: &str, arguments: &[&str]| v1::Profile {
        id: id.into(),
        name: name.into(),
        command: v1::ShellCommand {
            program: program.into(),
            arguments: arguments.iter().map(|value| (*value).into()).collect(),
            environment: Default::default(),
        },
    };
    let (default, mut profiles) = match info.pty {
        process_info::PtyFlavor::Conpty => (
            "powershell",
            vec![make("cmd", "Command Prompt", "cmd.exe", &["/D"])],
        ),
        process_info::PtyFlavor::Posix => (
            "login-shell",
            vec![
                make("bash", "bash", "bash", &[]),
                make("zsh", "zsh", "zsh", &[]),
                make("fish", "fish", "fish", &[]),
                make("pwsh", "PowerShell", "pwsh", &[]),
            ],
        ),
    };
    profiles.insert(
        0,
        v1::Profile {
            id: default.into(),
            name: if default == "powershell" {
                "PowerShell"
            } else {
                "Login Shell"
            }
            .into(),
            command: default_command,
        },
    );
    Ok(v1::ProfilesResponse {
        profiles,
        default: Some(default.into()),
    })
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
        v1::PROFILES => {
            let _: v1::ProfilesRequest = decode(payload)?;
            encode(profiles()?)
        }
        v1::START_PROFILE => {
            let input: v1::StartProfileRequest = decode(payload)?;
            let profiles = profiles()?;
            let selected = input
                .profile
                .as_ref()
                .or(profiles.default.as_ref())
                .ok_or("no default terminal profile")?;
            let command = &profiles
                .profiles
                .iter()
                .find(|profile| &profile.id == selected)
                .ok_or("unknown terminal profile")?
                .command;
            let start = v1::StartRequest {
                requested_id: input.requested_id,
                program: command.program.clone(),
                arguments: command.arguments.clone(),
                environment: command.environment.clone(),
                working_directory: input.working_directory,
                columns: input.columns,
                rows: input.rows,
            };
            run(
                v1::START,
                &serde_json::to_string(&start).map_err(|error| error.to_string())?,
                caller,
                workspace,
            )
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

/// 現在の拡張インスタンスに Host 能力のアダプターを設定する。
pub fn activate(host: Box<dyn TerminalHost>) {
    HOST.with(|slot| *slot.borrow_mut() = Some(host));
}

/// 拡張を明示的に停止するときにセッションを解放する。
pub fn deactivate() {
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
    HOST.with(|slot| *slot.borrow_mut() = None);
}

pub fn dispatch(
    operation: &str,
    payload: &str,
    session: &str,
    workspace: &str,
) -> Result<String, String> {
    if session.is_empty() || workspace.is_empty() {
        return Err("missing authenticated context".into());
    }
    serde_json::to_string(&run(operation, payload, session, workspace)?)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    struct FakeHost(Rc<RefCell<Vec<String>>>);
    impl TerminalHost for FakeHost {
        fn information(&self) -> Result<process_info::Information, String> {
            Ok(process_info::Information {
                pty: process_info::PtyFlavor::Conpty,
                default_shell: process_info::Command {
                    program: "shell".into(),
                    arguments: vec![],
                },
            })
        }
        fn spawn(&self, command: &sessions::SpawnRequest) -> Result<String, String> {
            assert_eq!(command.working_directory, ".");
            self.0.borrow_mut().push("spawn".into());
            Ok("pty-1".into())
        }
        fn attach(&self, _: &str) -> Result<(), String> {
            self.0.borrow_mut().push("attach".into());
            Ok(())
        }
        fn detach(&self, _: &str) -> Result<(), String> {
            self.0.borrow_mut().push("detach".into());
            Ok(())
        }
        fn write(&self, _: &str, bytes: &[u8]) -> Result<(), String> {
            assert_eq!(bytes, b"hello");
            self.0.borrow_mut().push("write".into());
            Ok(())
        }
        fn resize(&self, _: &str, _: sessions::Size) -> Result<(), String> {
            Ok(())
        }
        fn read(&self, _: &str, _: u64, _: u32, _: u32) -> Result<sessions::Output, String> {
            Ok(sessions::Output {
                bytes: b"ok".to_vec(),
                next: 2,
                lost_before: None,
                exit: None,
                read_error: None,
                write_error: None,
            })
        }
        fn kill(&self, _: &str) -> Result<(), String> {
            self.0.borrow_mut().push("kill".into());
            Ok(())
        }
        fn forget(&self, _: &str) -> Result<(), String> {
            self.0.borrow_mut().push("forget".into());
            Ok(())
        }
    }

    #[test]
    fn sessions_use_only_the_supplied_host_and_respect_workspace_context() {
        let calls = Rc::new(RefCell::new(Vec::new()));
        activate(Box::new(FakeHost(calls.clone())));
        let start = r#"{"profile":null,"working_directory":".","columns":80,"rows":24}"#;
        let result: v1::StartResponse = serde_json::from_str(
            &dispatch(v1::START_PROFILE, start, "client-a", "workspace-a").unwrap(),
        )
        .unwrap();
        let request =
            serde_json::json!({"id": result.id, "bytes": [104,101,108,108,111]}).to_string();
        assert!(dispatch(v1::WRITE, &request, "client-a", "workspace-b").is_err());
        dispatch(v1::WRITE, &request, "client-a", "workspace-a").unwrap();
        let read =
            serde_json::json!({"id": result.id, "offset": 0, "max_bytes": 8, "timeout_ms": 0})
                .to_string();
        let output: v1::ReadResponse =
            serde_json::from_str(&dispatch(v1::READ, &read, "client-a", "workspace-a").unwrap())
                .unwrap();
        assert_eq!(output.bytes, b"ok");
        deactivate();
        assert_eq!(
            &*calls.borrow(),
            &["spawn", "attach", "write", "kill", "detach"]
        );
    }
}
