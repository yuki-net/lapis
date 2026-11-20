//! 承認された拡張だけが使う Host 所有の PTY 能力。
use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
};

use client_api::WorkspaceId;
use extension_runtime::InvocationContext;
use extension_runtime::supervisor::HostCallHandler;
use process::pty::{PtyAttachmentId, PtyOwnerId, PtyService, PtySessionId, PtySize, PtySpawn};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::extensions::HostExtensions;

pub(crate) struct PtyBroker {
    extensions: Arc<HostExtensions>,
    sessions: local_process::pty::LocalPtyService,
    gate: Mutex<()>,
    resources: Arc<backend_state::ResourceGuard>,
    workspaces: Arc<Mutex<HashMap<PathBuf, WorkspaceId>>>,
}

impl PtyBroker {
    #[cfg(test)]
    pub(crate) fn new(extensions: Arc<HostExtensions>) -> Self {
        Self::with_workspaces(extensions, Arc::new(Mutex::new(HashMap::new())))
    }

    #[cfg(test)]
    pub(crate) fn with_workspaces(
        extensions: Arc<HostExtensions>,
        workspaces: Arc<Mutex<HashMap<PathBuf, WorkspaceId>>>,
    ) -> Self {
        Self::with_guard(
            extensions,
            workspaces,
            Arc::new(backend_state::ResourceGuard::default()),
        )
    }

    pub(crate) fn with_guard(
        extensions: Arc<HostExtensions>,
        workspaces: Arc<Mutex<HashMap<PathBuf, WorkspaceId>>>,
        resources: Arc<backend_state::ResourceGuard>,
    ) -> Self {
        Self {
            extensions,
            sessions: local_process::pty::LocalPtyService::default(),
            gate: Mutex::new(()),
            resources,
            workspaces,
        }
    }

    pub(crate) fn active_in(&self, root: &Path) -> Result<bool, String> {
        let _guard = self.gate.lock().map_err(|error| error.to_string())?;
        self.sessions
            .active_in(root)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn set_approval(
        &self,
        extension: &str,
        granted: bool,
        expected_digest: &str,
    ) -> Result<(), String> {
        let _guard = self.gate.lock().map_err(|error| error.to_string())?;
        self.extensions
            .set_pty_approval(extension, granted, expected_digest)?;
        if !granted {
            self.sessions
                .close_owner(&PtyOwnerId(extension.to_owned()))
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn session_count(&self, extension: &str) -> usize {
        self.sessions
            .session_count(&PtyOwnerId(extension.to_owned()))
            .unwrap()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Id {
    id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Write {
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

fn parse<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|_| "invalid PTY request".to_owned())
}

impl HostCallHandler for PtyBroker {
    fn call_in_context(
        &self,
        extension: &str,
        method: &str,
        payload: Value,
        context: Option<&InvocationContext>,
    ) -> Result<Value, String> {
        if method != "pty.v1.spawn" {
            return self.call(extension, method, payload);
        }
        let context = context.ok_or("PTY spawn requires an authenticated workspace")?;
        let mut command: PtySpawn = parse(payload)?;
        let path = Path::new(&command.cwd);
        if path
            .components()
            .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
        {
            return Err("PTY working directory must be workspace-relative".into());
        }
        let root = self
            .workspaces
            .lock()
            .map_err(|_| "workspace state unavailable")?
            .iter()
            .find(|(_, id)| id.as_str() == context.workspace)
            .map(|(root, _)| root.clone())
            .ok_or("workspace not found")?;
        let resolved = root
            .join(path)
            .canonicalize()
            .map_err(|_| "working directory not found")?;
        if !resolved.starts_with(&root) {
            return Err("working directory escapes workspace".into());
        }
        command.cwd = resolved
            .to_str()
            .ok_or("working directory is not UTF-8")?
            .to_owned();
        self.resources.with_path(&resolved, || {
            self.call(
                extension,
                method,
                serde_json::to_value(command).map_err(|error| error.to_string())?,
            )
        })
    }

    fn call(&self, extension: &str, method: &str, payload: Value) -> Result<Value, String> {
        if method == "pty.v1.read" {
            let value: Read = parse(payload)?;
            if value.timeout_ms > 1_000 {
                return Err("PTY read timeout is too long".into());
            }
            let owner = PtyOwnerId(extension.to_owned());
            let attachment = PtyAttachmentId("extension-host".to_owned());
            let id = PtySessionId(value.id);
            {
                let _guard = self.gate.lock().map_err(|error| error.to_string())?;
                if !self.extensions.pty_approved(extension)? {
                    return Err("PTY capability is not approved".into());
                }
            }
            self.sessions
                .wait_read(
                    &owner,
                    &id,
                    &attachment,
                    value.offset,
                    value.max_bytes as usize,
                    u64::from(value.timeout_ms),
                )
                .map_err(|error| error.to_string())?;
            let _guard = self.gate.lock().map_err(|error| error.to_string())?;
            if !self.extensions.pty_approved(extension)? {
                return Err("PTY capability is not approved".into());
            }
            let output = self
                .sessions
                .read(
                    &owner,
                    &id,
                    &attachment,
                    value.offset,
                    value.max_bytes as usize,
                )
                .map_err(|error| error.to_string())?;
            return serde_json::to_value(output).map_err(|error| error.to_string());
        }
        let _guard = self.gate.lock().map_err(|error| error.to_string())?;
        if !self.extensions.pty_approved(extension)? {
            return Err("PTY capability is not approved".into());
        }
        let owner = PtyOwnerId(extension.to_owned());
        let attachment = PtyAttachmentId("extension-host".to_owned());
        match method {
            "process.info.v1.get" => {
                if !payload.is_null() || !self.extensions.process_info_declared(extension)? {
                    return Err("process information contract is not declared".into());
                }
                Ok(json!({
                    "pty": "conpty",
                    "default_shell": {"program": "powershell.exe", "arguments": ["-NoLogo"]}
                }))
            }
            "pty.v1.spawn" => {
                let command: PtySpawn = parse(payload)?;
                let id = self
                    .sessions
                    .spawn(&owner, command)
                    .map_err(|error| error.to_string())?;
                Ok(json!(id.0))
            }
            "pty.v1.attach" | "pty.v1.detach" | "pty.v1.kill" | "pty.v1.forget" => {
                let value: Id = parse(payload)?;
                let id = PtySessionId(value.id);
                match method {
                    "pty.v1.attach" => self.sessions.attach(&owner, &id, attachment),
                    "pty.v1.detach" => self.sessions.detach(&owner, &id, &attachment),
                    "pty.v1.kill" => self.sessions.kill(&owner, &id, &attachment),
                    _ => self.sessions.forget(&owner, &id),
                }
                .map_err(|error| error.to_string())?;
                Ok(Value::Null)
            }
            "pty.v1.write" => {
                let value: Write = parse(payload)?;
                self.sessions
                    .write(&owner, &PtySessionId(value.id), &attachment, &value.bytes)
                    .map_err(|error| error.to_string())?;
                Ok(Value::Null)
            }
            "pty.v1.resize" => {
                let value: Resize = parse(payload)?;
                self.sessions
                    .resize(
                        &owner,
                        &PtySessionId(value.id),
                        &attachment,
                        PtySize {
                            columns: value.columns,
                            rows: value.rows,
                        },
                    )
                    .map_err(|error| error.to_string())?;
                Ok(Value::Null)
            }
            _ => Err("unknown Host capability".into()),
        }
    }

    fn release(&self, extension: Option<&str>) {
        let Ok(_guard) = self.gate.lock() else { return };
        let owners = extension
            .map(|id| vec![id.to_owned()])
            .unwrap_or_else(|| self.extensions.runtime_ids());
        for id in owners {
            if let Err(error) = self.sessions.close_owner(&PtyOwnerId(id)) {
                eprintln!("拡張の PTY を回収できません: {error}");
            }
        }
    }
}
