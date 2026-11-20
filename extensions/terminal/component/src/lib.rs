//! 実行方式に依存しない Terminal ロジックを WIT へ接続する。

#![allow(clippy::too_many_arguments)] // wit-bindgen の生成 ABI

wit_bindgen::generate!({ path: "wit", world: "terminal", generate_all });

use lapis::process_info::host as info;
use lapis::pty::sessions as pty;
use terminal_logic::{TerminalHost, process_info, sessions};

struct WitHost;

impl TerminalHost for WitHost {
    fn information(&self) -> Result<process_info::Information, String> {
        let value = info::get()?;
        Ok(process_info::Information {
            pty: match value.pty {
                info::PtyFlavor::Conpty => process_info::PtyFlavor::Conpty,
                info::PtyFlavor::Posix => process_info::PtyFlavor::Posix,
            },
            default_shell: process_info::Command {
                program: value.default_shell.program,
                arguments: value.default_shell.arguments,
            },
        })
    }
    fn spawn(&self, value: &sessions::SpawnRequest) -> Result<String, String> {
        pty::spawn(&pty::SpawnRequest {
            program: value.program.clone(),
            arguments: value.arguments.clone(),
            environment: value
                .environment
                .iter()
                .map(|item| pty::EnvironmentEntry {
                    name: item.name.clone(),
                    value: item.value.clone(),
                })
                .collect(),
            working_directory: value.working_directory.clone(),
            initial_size: pty::Size {
                columns: value.initial_size.columns,
                rows: value.initial_size.rows,
            },
        })
    }
    fn attach(&self, id: &str) -> Result<(), String> {
        pty::attach(id)
    }
    fn detach(&self, id: &str) -> Result<(), String> {
        pty::detach(id)
    }
    fn write(&self, id: &str, bytes: &[u8]) -> Result<(), String> {
        pty::write(id, bytes)
    }
    fn resize(&self, id: &str, value: sessions::Size) -> Result<(), String> {
        pty::resize(
            id,
            pty::Size {
                columns: value.columns,
                rows: value.rows,
            },
        )
    }
    fn read(
        &self,
        id: &str,
        offset: u64,
        max_bytes: u32,
        timeout_ms: u32,
    ) -> Result<sessions::Output, String> {
        let value = pty::read(id, offset, max_bytes, timeout_ms)?;
        Ok(sessions::Output {
            bytes: value.bytes,
            next: value.next,
            lost_before: value.lost_before,
            exit: value
                .exit
                .map(|exit| sessions::ExitStatus { code: exit.code }),
            read_error: value.read_error,
            write_error: value.write_error,
        })
    }
    fn kill(&self, id: &str) -> Result<(), String> {
        pty::kill(id)
    }
    fn forget(&self, id: &str) -> Result<(), String> {
        pty::forget(id)
    }
}

struct Terminal;
thread_local! {
    static INSTANCE: std::cell::RefCell<Option<terminal_logic::TerminalLogic>> = const { std::cell::RefCell::new(None) };
}
impl Guest for Terminal {
    fn activate() {
        INSTANCE.with(|instance| {
            *instance.borrow_mut() = Some(terminal_logic::TerminalLogic::new(Box::new(WitHost)));
        });
    }
    fn deactivate() {
        INSTANCE.with(|instance| {
            if let Some(mut logic) = instance.borrow_mut().take() {
                logic.deactivate();
            }
        });
    }
    fn dispatch(
        operation: String,
        payload: String,
        session: String,
        workspace: String,
    ) -> Result<String, String> {
        INSTANCE.with(|instance| {
            instance
                .borrow_mut()
                .as_mut()
                .ok_or("terminal is not active")?
                .dispatch(&operation, &payload, &session, &workspace)
        })
    }
}
export!(Terminal);
