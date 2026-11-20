//! Wasm の PTY v1 呼び出しを汎用 Host IPC へ変換する。
use std::collections::BTreeMap;

use crate::{
    WorkerState,
    process_info_bindings::lapis::process_info::host::{
        self as info, Command, Information, PtyFlavor,
    },
    pty_bindings::lapis::pty::sessions::{self, ExitStatus, Output, Size, SpawnRequest},
};
use serde_json::{Value, json};

impl WorkerState {
    fn host_call(&self, method: &str, value: Value) -> Result<Value, String> {
        self.broker
            .as_ref()
            .ok_or_else(|| "Host capability unavailable".to_owned())?(
            &self.extension,
            method,
            value,
        )
    }
    fn pty_empty(&self, method: &str, value: Value) -> Result<(), String> {
        self.host_call(method, value).map(|_| ())
    }
}

impl info::Host for WorkerState {
    fn get(&mut self) -> Result<Information, String> {
        let value = self.host_call("process.info.v1.get", Value::Null)?;
        let flavor = match value.get("pty").and_then(Value::as_str) {
            Some("conpty") => PtyFlavor::Conpty,
            Some("posix") => PtyFlavor::Posix,
            _ => return Err("invalid Host process information".into()),
        };
        let command = value
            .get("default_shell")
            .ok_or("invalid Host process information")?;
        let program = command
            .get("program")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && value.len() <= 4096)
            .ok_or("invalid Host process information")?
            .to_owned();
        let arguments: Vec<String> = serde_json::from_value(
            command
                .get("arguments")
                .cloned()
                .ok_or("invalid Host process information")?,
        )
        .map_err(|_| "invalid Host process information")?;
        if arguments.len() > 64 || arguments.iter().any(|arg| arg.len() > 4096) {
            return Err("invalid Host process information".into());
        }
        Ok(Information {
            pty: flavor,
            default_shell: Command { program, arguments },
        })
    }
}

impl sessions::Host for WorkerState {
    fn spawn(&mut self, command: SpawnRequest) -> Result<String, String> {
        let mut env = BTreeMap::new();
        for entry in command.environment {
            if env.insert(entry.name, entry.value).is_some() {
                return Err("duplicate environment name".into());
            }
        }
        let command = process::pty::PtySpawn {
            program: command.program,
            args: command.arguments,
            env,
            cwd: command.working_directory,
            size: process::pty::PtySize {
                columns: command.initial_size.columns,
                rows: command.initial_size.rows,
            },
        };
        let result = self.host_call(
            "pty.v1.spawn",
            serde_json::to_value(command).map_err(|e| e.to_string())?,
        )?;
        serde_json::from_value(result).map_err(|_| "invalid Host result".into())
    }
    fn attach(&mut self, id: String) -> Result<(), String> {
        self.pty_empty("pty.v1.attach", json!({"id":id}))
    }
    fn detach(&mut self, id: String) -> Result<(), String> {
        self.pty_empty("pty.v1.detach", json!({"id":id}))
    }
    fn write(&mut self, id: String, bytes: Vec<u8>) -> Result<(), String> {
        self.pty_empty("pty.v1.write", json!({"id":id,"bytes":bytes}))
    }
    fn resize(&mut self, id: String, size: Size) -> Result<(), String> {
        self.pty_empty(
            "pty.v1.resize",
            json!({"id":id,"columns":size.columns,"rows":size.rows}),
        )
    }
    fn read(
        &mut self,
        id: String,
        offset: u64,
        max_bytes: u32,
        timeout_ms: u32,
    ) -> Result<Output, String> {
        let result = self.host_call(
            "pty.v1.read",
            json!({"id":id,"offset":offset,"max_bytes":max_bytes,"timeout_ms":timeout_ms}),
        )?;
        let output: process::pty::PtyOutput =
            serde_json::from_value(result).map_err(|_| "invalid Host result".to_owned())?;
        Ok(Output {
            bytes: output.bytes,
            next: output.next,
            lost_before: output.lost_before,
            exit: output.exited.map(|exit| ExitStatus { code: exit.code }),
            read_error: output.read_error,
            write_error: output.write_error,
        })
    }
    fn kill(&mut self, id: String) -> Result<(), String> {
        self.pty_empty("pty.v1.kill", json!({"id":id}))
    }
    fn forget(&mut self, id: String) -> Result<(), String> {
        self.pty_empty("pty.v1.forget", json!({"id":id}))
    }
}
