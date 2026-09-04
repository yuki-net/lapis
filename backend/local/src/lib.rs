use std::{
    collections::HashMap,
    env, fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::UNIX_EPOCH,
    time::{Duration, Instant},
};

use lapis_app_services::{WorkspaceDialog, WorkspaceRepository};
use lapis_document::{DocumentError, DocumentRepository, FileData, FileFingerprint};
use lapis_editor_core::ExecutionId;
use lapis_git::{
    ChangeKind, ChangedFile, FileDiff, GitBackend, GitError, GitErrorKind, RepositoryStatus,
    TaskWorktree, WorktreeState,
};
use lapis_lsp::{
    CompletionItem, DefinitionTarget, Diagnostic, DiagnosticSeverity, LanguageServerBackend,
    LspError, LspPosition, LspRange,
};
use lapis_task_runner::{
    CodexExecutionSpec, ExecutionStatus, RunnerUpdate, TaskBackend, TaskControl, TaskError,
    TaskEvent, TaskRecord, run_codex_app_server,
};
use lapis_terminal::{TerminalBackend, TerminalError, TerminalEvent, TerminalId};
use lapis_workspace::{
    DocumentViewState, FileEntry, FileEntryKind, SearchError, SearchHit, WorkspaceError,
    WorkspaceSearchBackend, WorkspaceSnapshot, WorkspaceStateRepository,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Default)]
pub struct ConnectionGate(Arc<AtomicBool>);

impl ConnectionGate {
    pub fn connected() -> Self {
        Self(Arc::new(AtomicBool::new(true)))
    }

    pub fn connect(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn disconnect(&self) {
        self.0.store(false, Ordering::Release);
    }

    pub fn is_connected(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

pub struct LoopbackBackend<T> {
    inner: Arc<T>,
    gate: ConnectionGate,
}

impl<T> Clone for LoopbackBackend<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            gate: self.gate.clone(),
        }
    }
}

impl<T> LoopbackBackend<T> {
    pub fn new(inner: Arc<T>, gate: ConnectionGate) -> Self {
        Self { inner, gate }
    }

    pub fn gate(&self) -> &ConnectionGate {
        &self.gate
    }
}

struct LiveTerminal {
    master: Box<dyn portable_pty::MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn portable_pty::Child + Send>,
    receiver: mpsc::Receiver<TerminalEvent>,
    exited: bool,
}

#[derive(Clone, Default)]
pub struct LocalTerminalBackend {
    terminals: Arc<Mutex<HashMap<String, LiveTerminal>>>,
    next_id: Arc<std::sync::atomic::AtomicU64>,
}

impl LocalTerminalBackend {
    fn lock(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, HashMap<String, LiveTerminal>>, TerminalError> {
        self.terminals
            .lock()
            .map_err(|_| TerminalError::new("Terminal state lock failed"))
    }
}

fn terminal_display_text(input: &str) -> String {
    #[derive(Clone, Copy)]
    enum EscapeState {
        Text,
        Escape,
        Csi,
        Osc,
        OscEscape,
    }
    let mut state = EscapeState::Text;
    let mut output = String::new();
    for character in input.chars() {
        state = match (state, character) {
            (EscapeState::Text, '\u{1b}') => EscapeState::Escape,
            (EscapeState::Text, '\r') => EscapeState::Text,
            (EscapeState::Text, '\u{8}') => {
                output.pop();
                EscapeState::Text
            }
            (EscapeState::Text, value) => {
                output.push(value);
                EscapeState::Text
            }
            (EscapeState::Escape, '[') => EscapeState::Csi,
            (EscapeState::Escape, ']') => EscapeState::Osc,
            (EscapeState::Escape, _) => EscapeState::Text,
            (EscapeState::Csi, value) if ('@'..='~').contains(&value) => EscapeState::Text,
            (EscapeState::Csi, _) => EscapeState::Csi,
            (EscapeState::Osc, '\u{7}') => EscapeState::Text,
            (EscapeState::Osc, '\u{1b}') => EscapeState::OscEscape,
            (EscapeState::Osc, _) => EscapeState::Osc,
            (EscapeState::OscEscape, '\\') => EscapeState::Text,
            (EscapeState::OscEscape, _) => EscapeState::Osc,
        };
    }
    output
}

impl TerminalBackend for LocalTerminalBackend {
    fn start(&self, cwd: &Path, columns: u16, rows: u16) -> Result<TerminalId, TerminalError> {
        let pty_system = portable_pty::native_pty_system();
        let pair = pty_system
            .openpty(portable_pty::PtySize {
                rows,
                cols: columns,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| TerminalError::new(error.to_string()))?;
        #[cfg(windows)]
        let mut command = portable_pty::CommandBuilder::new("powershell.exe");
        #[cfg(not(windows))]
        let mut command = portable_pty::CommandBuilder::new(
            env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned()),
        );
        #[cfg(windows)]
        command.args(["-NoLogo"]);
        command.cwd(cwd);
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| TerminalError::new(error.to_string()))?;
        let mut writer = pair
            .master
            .take_writer()
            .map_err(|error| TerminalError::new(error.to_string()))?;
        // ConPTY asks the terminal emulator for its initial cursor position.  The
        // current UI renders the byte stream directly, so answer this standard
        // query here instead of exposing it as command input.
        writer
            .write_all(b"\x1b[1;1R")
            .and_then(|()| writer.flush())
            .map_err(|error| TerminalError::new(error.to_string()))?;
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| TerminalError::new(error.to_string()))?;
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let mut buffer = [0; 4096];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(length) => {
                        let text =
                            terminal_display_text(&String::from_utf8_lossy(&buffer[..length]));
                        if sender.send(TerminalEvent::Output(text)).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(TerminalEvent::Failed(error.to_string()));
                        break;
                    }
                }
            }
        });
        let id = TerminalId::new(format!(
            "terminal-{}",
            self.next_id.fetch_add(1, Ordering::Relaxed) + 1
        ));
        self.lock()?.insert(
            id.as_str().to_owned(),
            LiveTerminal {
                master: pair.master,
                writer,
                child,
                receiver,
                exited: false,
            },
        );
        Ok(id)
    }

    fn input(&self, id: &TerminalId, bytes: &[u8]) -> Result<(), TerminalError> {
        let mut terminals = self.lock()?;
        let terminal = terminals
            .get_mut(id.as_str())
            .ok_or_else(|| TerminalError::new("Terminal not found"))?;
        terminal
            .writer
            .write_all(bytes)
            .and_then(|()| terminal.writer.flush())
            .map_err(|error| TerminalError::new(error.to_string()))
    }

    fn resize(&self, id: &TerminalId, columns: u16, rows: u16) -> Result<(), TerminalError> {
        let mut terminals = self.lock()?;
        let terminal = terminals
            .get_mut(id.as_str())
            .ok_or_else(|| TerminalError::new("Terminal not found"))?;
        terminal
            .master
            .resize(portable_pty::PtySize {
                rows,
                cols: columns,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| TerminalError::new(error.to_string()))
    }

    fn poll(&self, id: &TerminalId) -> Result<Vec<TerminalEvent>, TerminalError> {
        let mut terminals = self.lock()?;
        let terminal = terminals
            .get_mut(id.as_str())
            .ok_or_else(|| TerminalError::new("Terminal not found"))?;
        let mut events = terminal.receiver.try_iter().collect::<Vec<_>>();
        if !terminal.exited
            && let Some(status) = terminal
                .child
                .try_wait()
                .map_err(|error| TerminalError::new(error.to_string()))?
        {
            terminal.exited = true;
            events.push(TerminalEvent::Exited {
                code: Some(status.exit_code() as i32),
            });
        }
        Ok(events)
    }

    fn terminate(&self, id: &TerminalId) -> Result<(), TerminalError> {
        let mut terminals = self.lock()?;
        let terminal = terminals
            .get_mut(id.as_str())
            .ok_or_else(|| TerminalError::new("Terminal not found"))?;
        terminal
            .child
            .kill()
            .map_err(|error| TerminalError::new(error.to_string()))?;
        terminal.exited = true;
        Ok(())
    }
}

struct LspProcess {
    child: std::process::Child,
    writer: std::process::ChildStdin,
    receiver: mpsc::Receiver<serde_json::Value>,
    next_request: u64,
    revisions: HashMap<PathBuf, lapis_document::Revision>,
    diagnostics: Vec<Diagnostic>,
    diagnostics_dirty: bool,
}

#[derive(Default)]
pub struct LocalLspBackend {
    process: Mutex<Option<LspProcess>>,
}

impl LocalLspBackend {
    fn with_process<T>(
        &self,
        operation: impl FnOnce(&mut LspProcess) -> Result<T, LspError>,
    ) -> Result<T, LspError> {
        let mut guard = self
            .process
            .lock()
            .map_err(|_| LspError::new("Language server state lock failed"))?;
        let process = guard
            .as_mut()
            .ok_or_else(|| LspError::new("Language server is not running"))?;
        operation(process)
    }
}

impl LanguageServerBackend for LocalLspBackend {
    fn start(&self, workspace: &Path) -> Result<(), LspError> {
        let mut guard = self
            .process
            .lock()
            .map_err(|_| LspError::new("Language server state lock failed"))?;
        if guard.is_some() {
            return Ok(());
        }
        let version = Command::new("rust-analyzer")
            .arg("--version")
            .output()
            .map_err(|error| LspError::new(format!("rust-analyzer is unavailable: {error}")))?;
        if !version.status.success() {
            return Err(LspError::new(format!(
                "rust-analyzer is unavailable: {}",
                String::from_utf8_lossy(&version.stderr).trim()
            )));
        }
        let mut child = Command::new("rust-analyzer")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .current_dir(workspace)
            .spawn()
            .map_err(|error| LspError::new(format!("rust-analyzer start failed: {error}")))?;
        let writer = child
            .stdin
            .take()
            .ok_or_else(|| LspError::new("rust-analyzer stdin is unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| LspError::new("rust-analyzer stdout is unavailable"))?;
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || read_lsp_messages(stdout, sender));
        let mut process = LspProcess {
            child,
            writer,
            receiver,
            next_request: 1,
            revisions: HashMap::new(),
            diagnostics: Vec::new(),
            diagnostics_dirty: false,
        };
        let root_uri = file_uri(workspace);
        let id = lsp_request(
            &mut process,
            "initialize",
            serde_json::json!({
                "processId": std::process::id(),
                "rootUri": root_uri,
                "capabilities": {
                    "textDocument": {
                        "synchronization": { "didSave": true },
                        "completion": { "completionItem": { "snippetSupport": false } },
                        "definition": {},
                        "diagnostic": { "dynamicRegistration": false, "relatedDocumentSupport": false }
                    }
                }
            }),
        )?;
        let response = lsp_wait_response(&mut process, id, Duration::from_secs(20))?;
        if let Some(error) = response.get("error") {
            let _ = process.child.kill();
            return Err(LspError::new(format!(
                "rust-analyzer initialize failed: {error}"
            )));
        }
        lsp_notify(&mut process, "initialized", serde_json::json!({}))?;
        *guard = Some(process);
        Ok(())
    }

    fn did_open(
        &self,
        path: &Path,
        text: &str,
        revision: lapis_document::Revision,
    ) -> Result<(), LspError> {
        self.with_process(|process| {
            let path = normalize_lsp_path(path);
            process.revisions.insert(path.clone(), revision);
            process.diagnostics_dirty = true;
            lsp_notify(
                process,
                "textDocument/didOpen",
                serde_json::json!({ "textDocument": {
                    "uri": file_uri(&path), "languageId": "rust",
                    "version": revision.number(), "text": text
                }}),
            )
        })
    }

    fn did_change(
        &self,
        path: &Path,
        text: &str,
        revision: lapis_document::Revision,
    ) -> Result<(), LspError> {
        self.with_process(|process| {
            let path = normalize_lsp_path(path);
            process.revisions.insert(path.clone(), revision);
            process.diagnostics_dirty = true;
            lsp_notify(
                process,
                "textDocument/didChange",
                serde_json::json!({
                    "textDocument": { "uri": file_uri(&path), "version": revision.number() },
                    "contentChanges": [{ "text": text }]
                }),
            )
        })
    }

    fn diagnostics(&self) -> Result<Vec<Diagnostic>, LspError> {
        self.with_process(|process| {
            lsp_drain_notifications(process);
            if process.diagnostics_dirty {
                process.diagnostics_dirty = false;
                let documents = process
                    .revisions
                    .iter()
                    .map(|(path, revision)| (path.clone(), *revision))
                    .collect::<Vec<_>>();
                for (path, revision) in documents {
                    let id = lsp_request(
                        process,
                        "textDocument/diagnostic",
                        serde_json::json!({
                            "textDocument": { "uri": file_uri(&path) }
                        }),
                    )?;
                    let response = lsp_wait_response(process, id, Duration::from_secs(8))?;
                    if let Some(error) = response.get("error") {
                        return Err(LspError::new(format!("LSP diagnostics failed: {error}")));
                    }
                    let items = response
                        .pointer("/result/items")
                        .and_then(|value| value.as_array())
                        .map(Vec::as_slice);
                    replace_diagnostics(process, &path, revision, items);
                }
            }
            Ok(process.diagnostics.clone())
        })
    }

    fn completion(
        &self,
        path: &Path,
        position: LspPosition,
        revision: lapis_document::Revision,
    ) -> Result<Vec<CompletionItem>, LspError> {
        self.with_process(|process| {
            let path = normalize_lsp_path(path);
            if process.revisions.get(&path) != Some(&revision) {
                return Ok(Vec::new());
            }
            let id = lsp_request(
                process,
                "textDocument/completion",
                text_document_position(&path, position),
            )?;
            let response = lsp_wait_response(process, id, Duration::from_secs(8))?;
            if let Some(error) = response.get("error") {
                return Err(LspError::new(format!("LSP completion failed: {error}")));
            }
            let result = response
                .get("result")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let items = result
                .as_array()
                .or_else(|| result.get("items").and_then(|value| value.as_array()));
            Ok(items
                .into_iter()
                .flatten()
                .filter_map(|item| {
                    let label = item.get("label")?.as_str()?.to_owned();
                    Some(CompletionItem {
                        insert_text: item
                            .get("insertText")
                            .and_then(|value| value.as_str())
                            .unwrap_or(&label)
                            .to_owned(),
                        detail: item
                            .get("detail")
                            .and_then(|value| value.as_str())
                            .map(str::to_owned),
                        label,
                    })
                })
                .collect())
        })
    }

    fn definition(
        &self,
        path: &Path,
        position: LspPosition,
        revision: lapis_document::Revision,
    ) -> Result<Option<DefinitionTarget>, LspError> {
        self.with_process(|process| {
            let path = normalize_lsp_path(path);
            if process.revisions.get(&path) != Some(&revision) {
                return Ok(None);
            }
            let id = lsp_request(
                process,
                "textDocument/definition",
                text_document_position(&path, position),
            )?;
            let response = lsp_wait_response(process, id, Duration::from_secs(8))?;
            let result = response
                .get("result")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let location = result
                .as_array()
                .and_then(|items| items.first())
                .unwrap_or(&result);
            Ok(parse_location(location))
        })
    }

    fn shutdown(&self) -> Result<(), LspError> {
        let mut guard = self
            .process
            .lock()
            .map_err(|_| LspError::new("Language server state lock failed"))?;
        let Some(mut process) = guard.take() else {
            return Ok(());
        };
        let id = lsp_request(&mut process, "shutdown", serde_json::Value::Null)?;
        let _ = lsp_wait_response(&mut process, id, Duration::from_secs(5));
        let _ = lsp_notify(&mut process, "exit", serde_json::Value::Null);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if process.child.try_wait().ok().flatten().is_some() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = process.child.kill();
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }
}

fn read_lsp_messages(stdout: std::process::ChildStdout, sender: mpsc::Sender<serde_json::Value>) {
    let mut reader = BufReader::new(stdout);
    loop {
        let mut content_length = None;
        loop {
            let mut header = String::new();
            match reader.read_line(&mut header) {
                Ok(0) | Err(_) => return,
                Ok(_) if header == "\r\n" || header == "\n" => break,
                Ok(_) => {
                    if let Some(value) = header
                        .strip_prefix("Content-Length:")
                        .and_then(|value| value.trim().parse::<usize>().ok())
                    {
                        content_length = Some(value);
                    }
                }
            }
        }
        let Some(length) = content_length else {
            continue;
        };
        let mut body = vec![0; length];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        let Ok(message) = serde_json::from_slice(&body) else {
            continue;
        };
        if sender.send(message).is_err() {
            return;
        }
    }
}

fn lsp_write(process: &mut LspProcess, value: &serde_json::Value) -> Result<(), LspError> {
    let bytes = serde_json::to_vec(value).map_err(|error| LspError::new(error.to_string()))?;
    write!(process.writer, "Content-Length: {}\r\n\r\n", bytes.len())
        .and_then(|()| process.writer.write_all(&bytes))
        .and_then(|()| process.writer.flush())
        .map_err(|error| LspError::new(error.to_string()))
}

fn lsp_request(
    process: &mut LspProcess,
    method: &str,
    params: serde_json::Value,
) -> Result<u64, LspError> {
    let id = process.next_request;
    process.next_request += 1;
    lsp_write(
        process,
        &serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params
        }),
    )?;
    Ok(id)
}

fn lsp_notify(
    process: &mut LspProcess,
    method: &str,
    params: serde_json::Value,
) -> Result<(), LspError> {
    lsp_write(
        process,
        &serde_json::json!({
            "jsonrpc": "2.0", "method": method, "params": params
        }),
    )
}

fn lsp_wait_response(
    process: &mut LspProcess,
    request_id: u64,
    timeout: Duration,
) -> Result<serde_json::Value, LspError> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(LspError::new(format!("LSP request {request_id} timed out")));
        }
        let message = process
            .receiver
            .recv_timeout(remaining)
            .map_err(|error| LspError::new(format!("LSP response failed: {error}")))?;
        if message.get("id").and_then(|value| value.as_u64()) == Some(request_id) {
            return Ok(message);
        }
        lsp_handle_incoming(process, &message);
    }
}

fn lsp_drain_notifications(process: &mut LspProcess) {
    while let Ok(message) = process.receiver.try_recv() {
        lsp_handle_incoming(process, &message);
    }
}

fn lsp_handle_incoming(process: &mut LspProcess, message: &serde_json::Value) {
    if message.get("id").is_some()
        && let Some(method) = message.get("method").and_then(|value| value.as_str())
    {
        let result = match method {
            "workspace/configuration" => message
                .pointer("/params/items")
                .and_then(|value| value.as_array())
                .map(|items| serde_json::json!(vec![serde_json::Value::Null; items.len()]))
                .unwrap_or_else(|| serde_json::json!([])),
            "workspace/workspaceFolders" => serde_json::Value::Null,
            "client/registerCapability"
            | "client/unregisterCapability"
            | "window/workDoneProgress/create" => serde_json::Value::Null,
            "workspace/diagnostic/refresh" => {
                process.diagnostics_dirty = true;
                serde_json::Value::Null
            }
            _ => {
                let _ = lsp_write(
                    process,
                    &serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": message.get("id").cloned().unwrap_or(serde_json::Value::Null),
                        "error": { "code": -32601, "message": format!("Unsupported server request: {method}") }
                    }),
                );
                return;
            }
        };
        let _ = lsp_write(
            process,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": message.get("id").cloned().unwrap_or(serde_json::Value::Null),
                "result": result
            }),
        );
        return;
    }
    if message.get("method").and_then(|value| value.as_str())
        != Some("textDocument/publishDiagnostics")
    {
        return;
    }
    let Some(params) = message.get("params") else {
        return;
    };
    let Some(path) = params
        .get("uri")
        .and_then(|value| value.as_str())
        .map(path_from_file_uri)
    else {
        return;
    };
    let Some(revision) = process.revisions.get(&path).copied() else {
        return;
    };
    let items = params
        .get("diagnostics")
        .and_then(|value| value.as_array())
        .map(Vec::as_slice);
    replace_diagnostics(process, &path, revision, items);
}

fn replace_diagnostics(
    process: &mut LspProcess,
    path: &Path,
    revision: lapis_document::Revision,
    items: Option<&[serde_json::Value]>,
) {
    process
        .diagnostics
        .retain(|diagnostic| diagnostic.path != path);
    process
        .diagnostics
        .extend(items.into_iter().flatten().filter_map(|item| {
            Some(Diagnostic {
                path: path.to_owned(),
                range: parse_lsp_range(item.get("range")?)?,
                severity: match item
                    .get("severity")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(3)
                {
                    1 => DiagnosticSeverity::Error,
                    2 => DiagnosticSeverity::Warning,
                    4 => DiagnosticSeverity::Hint,
                    _ => DiagnosticSeverity::Information,
                },
                message: item.get("message")?.as_str()?.to_owned(),
                revision,
            })
        }));
}

fn parse_lsp_range(value: &serde_json::Value) -> Option<LspRange> {
    Some(LspRange {
        start: parse_lsp_position(value.get("start")?)?,
        end: parse_lsp_position(value.get("end")?)?,
    })
}

fn parse_lsp_position(value: &serde_json::Value) -> Option<LspPosition> {
    Some(LspPosition {
        line: value.get("line")?.as_u64()?.try_into().ok()?,
        utf16_column: value.get("character")?.as_u64()?.try_into().ok()?,
    })
}

fn text_document_position(path: &Path, position: LspPosition) -> serde_json::Value {
    serde_json::json!({
        "textDocument": { "uri": file_uri(path) },
        "position": { "line": position.line, "character": position.utf16_column }
    })
}

fn parse_location(value: &serde_json::Value) -> Option<DefinitionTarget> {
    let uri = value
        .get("uri")
        .or_else(|| value.get("targetUri"))?
        .as_str()?;
    let range = value
        .get("range")
        .or_else(|| value.get("targetSelectionRange"))?;
    Some(DefinitionTarget {
        path: path_from_file_uri(uri),
        range: parse_lsp_range(range)?,
    })
}

fn file_uri(path: &Path) -> String {
    let normalized = normalize_lsp_path(path)
        .to_string_lossy()
        .replace('\\', "/");
    let mut encoded = String::with_capacity(normalized.len());
    for byte in normalized.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/' | b':') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    if encoded.starts_with('/') {
        format!("file://{encoded}")
    } else {
        format!("file:///{encoded}")
    }
}

fn path_from_file_uri(uri: &str) -> PathBuf {
    let value = uri
        .strip_prefix("file:///")
        .or_else(|| uri.strip_prefix("file://"))
        .unwrap_or(uri);
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_digit(bytes[index + 1]), hex_digit(bytes[index + 2]))
        {
            decoded.push(high * 16 + low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    let decoded = String::from_utf8_lossy(&decoded).replace('/', std::path::MAIN_SEPARATOR_STR);
    normalize_lsp_path(Path::new(&decoded))
}

fn normalize_lsp_path(path: &Path) -> PathBuf {
    let absolute = path.canonicalize().unwrap_or_else(|_| path.to_owned());
    #[cfg(windows)]
    {
        let value = absolute.to_string_lossy();
        if let Some(value) = value.strip_prefix(r"\\?\") {
            return PathBuf::from(value);
        }
    }
    absolute
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[derive(Default)]
pub struct LocalWorkspaceRepository;

impl DocumentRepository for LocalWorkspaceRepository {
    fn read_file(&self, path: &Path) -> Result<FileData, DocumentError> {
        let bytes = fs::read(path).map_err(|error| DocumentError::io(error.to_string()))?;
        let fingerprint = fingerprint_for(path, &bytes)?;
        Ok(FileData { bytes, fingerprint })
    }

    fn write_file(
        &self,
        path: &Path,
        content: &[u8],
        expected: Option<&FileFingerprint>,
    ) -> Result<FileFingerprint, DocumentError> {
        let actual = self.fingerprint(path)?;
        match (expected, actual.as_ref()) {
            (Some(expected), Some(actual)) if expected != actual => {
                return Err(DocumentError::conflict(
                    "保存前に外部変更を検出しました。再読み込みまたは差分確認が必要です",
                ));
            }
            (Some(_), None) => {
                return Err(DocumentError::conflict(
                    "保存前にファイルが削除されました。確認なしに再作成しません",
                ));
            }
            (None, Some(_)) => {
                return Err(DocumentError::conflict(
                    "保存先に既存ファイルがあります。確認なしに上書きしません",
                ));
            }
            _ => {}
        }

        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).map_err(|error| DocumentError::io(error.to_string()))?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)
            .map_err(|error| DocumentError::io(error.to_string()))?;
        temporary
            .write_all(content)
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|error| DocumentError::io(error.to_string()))?;
        temporary
            .persist(path)
            .map_err(|error| DocumentError::io(error.error.to_string()))?;
        self.fingerprint(path)?
            .ok_or_else(|| DocumentError::io("保存後のファイルを確認できません"))
    }

    fn fingerprint(&self, path: &Path) -> Result<Option<FileFingerprint>, DocumentError> {
        match fs::read(path) {
            Ok(bytes) => fingerprint_for(path, &bytes).map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(DocumentError::io(error.to_string())),
        }
    }
}

impl WorkspaceRepository for LocalWorkspaceRepository {
    fn list_tree(&self, root: &Path) -> Result<Vec<FileEntry>, WorkspaceError> {
        if !root.is_dir() {
            return Err(WorkspaceError::new(format!(
                "Workspaceフォルダーが存在しません: {}",
                root.display()
            )));
        }
        let mut entries = Vec::new();
        collect_tree(root, root, 0, &mut entries)?;
        Ok(entries)
    }
}

impl<T: DocumentRepository> DocumentRepository for LoopbackBackend<T> {
    fn read_file(&self, path: &Path) -> Result<FileData, DocumentError> {
        if !self.gate.is_connected() {
            return Err(DocumentError::io("loopback workspace is disconnected"));
        }
        self.inner.read_file(path)
    }

    fn write_file(
        &self,
        path: &Path,
        content: &[u8],
        expected: Option<&FileFingerprint>,
    ) -> Result<FileFingerprint, DocumentError> {
        if !self.gate.is_connected() {
            return Err(DocumentError::io("loopback workspace is disconnected"));
        }
        self.inner.write_file(path, content, expected)
    }

    fn fingerprint(&self, path: &Path) -> Result<Option<FileFingerprint>, DocumentError> {
        if !self.gate.is_connected() {
            return Err(DocumentError::io("loopback workspace is disconnected"));
        }
        self.inner.fingerprint(path)
    }
}

impl<T: WorkspaceRepository> WorkspaceRepository for LoopbackBackend<T> {
    fn list_tree(&self, root: &Path) -> Result<Vec<FileEntry>, WorkspaceError> {
        if !self.gate.is_connected() {
            return Err(WorkspaceError::new("loopback workspace is disconnected"));
        }
        self.inner.list_tree(root)
    }
}

impl<T: WorkspaceStateRepository> WorkspaceStateRepository for LoopbackBackend<T> {
    fn load(&self) -> Result<Option<WorkspaceSnapshot>, WorkspaceError> {
        if !self.gate.is_connected() {
            return Err(WorkspaceError::new("loopback workspace is disconnected"));
        }
        self.inner.load()
    }

    fn save(&self, snapshot: &WorkspaceSnapshot) -> Result<(), WorkspaceError> {
        if !self.gate.is_connected() {
            return Err(WorkspaceError::new("loopback workspace is disconnected"));
        }
        self.inner.save(snapshot)
    }
}

#[derive(Default)]
pub struct LocalWorkspaceSearchBackend;

impl WorkspaceSearchBackend for LocalWorkspaceSearchBackend {
    fn search(
        &self,
        root: &Path,
        query: &str,
        cancelled: &AtomicBool,
    ) -> Result<Vec<SearchHit>, SearchError> {
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let mut hits = Vec::new();
        search_directory(root, root, query, cancelled, &mut hits)?;
        Ok(hits)
    }
}

impl<T: WorkspaceSearchBackend> WorkspaceSearchBackend for LoopbackBackend<T> {
    fn search(
        &self,
        root: &Path,
        query: &str,
        cancelled: &AtomicBool,
    ) -> Result<Vec<SearchHit>, SearchError> {
        if !self.gate.is_connected() {
            return Err(SearchError::new("loopback workspace is disconnected"));
        }
        self.inner.search(root, query, cancelled)
    }
}

fn search_directory(
    root: &Path,
    directory: &Path,
    query: &str,
    cancelled: &AtomicBool,
    hits: &mut Vec<SearchHit>,
) -> Result<(), SearchError> {
    if cancelled.load(Ordering::Relaxed) || hits.len() >= 2_000 {
        return Ok(());
    }
    let entries = fs::read_dir(directory).map_err(|error| SearchError::new(error.to_string()))?;
    for entry in entries.flatten() {
        if cancelled.load(Ordering::Relaxed) || hits.len() >= 2_000 {
            break;
        }
        let name = entry.file_name();
        if matches!(name.to_str(), Some(".git" | "target" | ".DS_Store")) {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            search_directory(root, &path, query, cancelled, hits)?;
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.len() > 2 * 1024 * 1024 {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        for (line_index, line) in text.lines().enumerate() {
            for (column, _) in line.match_indices(query) {
                hits.push(SearchHit {
                    path: path.strip_prefix(root).unwrap_or(&path).to_owned(),
                    line: line_index + 1,
                    utf8_column: column + 1,
                    preview: line.trim().chars().take(240).collect(),
                });
                if hits.len() >= 2_000 {
                    return Ok(());
                }
            }
        }
    }
    Ok(())
}

fn collect_tree(
    root: &Path,
    directory: &Path,
    depth: usize,
    output: &mut Vec<FileEntry>,
) -> Result<(), WorkspaceError> {
    let read_dir =
        fs::read_dir(directory).map_err(|error| WorkspaceError::new(error.to_string()))?;
    let mut children = read_dir
        .filter_map(Result::ok)
        .filter(|entry| {
            !matches!(
                entry.file_name().to_str(),
                Some(".git" | "target" | ".DS_Store")
            )
        })
        .collect::<Vec<_>>();
    children.sort_by(|left, right| {
        let left_dir = left.file_type().is_ok_and(|kind| kind.is_dir());
        let right_dir = right.file_type().is_ok_and(|kind| kind.is_dir());
        right_dir
            .cmp(&left_dir)
            .then_with(|| left.file_name().cmp(&right.file_name()))
    });

    for child in children {
        let path = child.path();
        let file_type = child
            .file_type()
            .map_err(|error| WorkspaceError::new(error.to_string()))?;
        let kind = if file_type.is_dir() {
            FileEntryKind::Directory
        } else {
            FileEntryKind::File
        };
        output.push(FileEntry {
            relative_path: path.strip_prefix(root).unwrap_or(&path).to_owned(),
            path: path.clone(),
            kind,
            depth,
        });
        if file_type.is_dir() && !file_type.is_symlink() {
            collect_tree(root, &path, depth.saturating_add(1), output)?;
        }
    }
    Ok(())
}

fn fingerprint_for(path: &Path, bytes: &[u8]) -> Result<FileFingerprint, DocumentError> {
    let metadata = fs::metadata(path).map_err(|error| DocumentError::io(error.to_string()))?;
    let modified_nanos = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos());
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    Ok(FileFingerprint::new(metadata.len(), modified_nanos, hash))
}

#[derive(Default)]
pub struct NativeWorkspaceDialog;

impl WorkspaceDialog for NativeWorkspaceDialog {
    fn choose_workspace_path(&self) -> Option<PathBuf> {
        rfd::FileDialog::new().pick_folder()
    }

    fn choose_file_path(&self) -> Option<PathBuf> {
        rfd::FileDialog::new().pick_file()
    }

    fn choose_save_path(&self, suggested_name: &str) -> Option<PathBuf> {
        rfd::FileDialog::new()
            .set_file_name(suggested_name)
            .save_file()
    }
}

pub struct LocalWorkspaceStateRepository {
    path: PathBuf,
}

impl LocalWorkspaceStateRepository {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn user_default() -> Self {
        let base = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir);
        Self::new(base.join("Lapis").join("session-v1.txt"))
    }
}

impl WorkspaceStateRepository for LocalWorkspaceStateRepository {
    fn load(&self) -> Result<Option<WorkspaceSnapshot>, WorkspaceError> {
        let mut input = match fs::File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(WorkspaceError::new(error.to_string())),
        };
        let mut text = String::new();
        input
            .read_to_string(&mut text)
            .map_err(|error| WorkspaceError::new(error.to_string()))?;
        parse_snapshot(&text).map(Some)
    }

    fn save(&self, snapshot: &WorkspaceSnapshot) -> Result<(), WorkspaceError> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).map_err(|error| WorkspaceError::new(error.to_string()))?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)
            .map_err(|error| WorkspaceError::new(error.to_string()))?;
        temporary
            .write_all(format_snapshot(snapshot).as_bytes())
            .and_then(|()| temporary.as_file().sync_all())
            .map_err(|error| WorkspaceError::new(error.to_string()))?;
        temporary
            .persist(&self.path)
            .map_err(|error| WorkspaceError::new(error.error.to_string()))?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct LocalTaskBackend {
    root: PathBuf,
    codex_executable: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct TaskWorkerSpec {
    root: PathBuf,
    record: TaskRecord,
    runner: CodexExecutionSpec,
}

impl LocalTaskBackend {
    pub fn new(root: PathBuf, codex_executable: PathBuf) -> Self {
        Self {
            root,
            codex_executable,
        }
    }

    pub fn user_default() -> Self {
        let base = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir);
        let executable = env::var_os("LAPIS_CODEX_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(default_codex_executable);
        Self::new(base.join("Lapis").join("tasks-v1"), executable)
    }

    fn record_path(&self, execution_id: &ExecutionId) -> PathBuf {
        self.root
            .join(format!("execution-{}.json", execution_id.as_str()))
    }

    fn control_path(&self, execution_id: &ExecutionId) -> PathBuf {
        self.root
            .join(format!("control-{}.json", execution_id.as_str()))
    }

    fn spec_path(&self, execution_id: &ExecutionId) -> PathBuf {
        self.root
            .join(format!("start-{}.json", execution_id.as_str()))
    }

    fn persist_record(&self, record: &TaskRecord) -> Result<(), TaskError> {
        atomic_json_write(&self.record_path(&record.execution.id), record)
    }

    fn take_control(&self, execution_id: &ExecutionId) -> Result<Option<TaskControl>, TaskError> {
        let path = self.control_path(execution_id);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(TaskError::new(error.to_string())),
        };
        let control = serde_json::from_slice(&bytes)?;
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(TaskError::new(error.to_string())),
        }
        Ok(Some(control))
    }
}

impl TaskBackend for LocalTaskBackend {
    fn load(&self) -> Result<Vec<TaskRecord>, TaskError> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(TaskError::new(error.to_string())),
        };
        let mut records = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| TaskError::new(error.to_string()))?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with("execution-") || !name.ends_with(".json") {
                continue;
            }
            let bytes = fs::read(entry.path())?;
            records.push(serde_json::from_slice(&bytes)?);
        }
        records.sort_by_key(|record: &TaskRecord| record.task.created_at_ms);
        records.reverse();
        Ok(records)
    }

    fn start(&self, record: &TaskRecord) -> Result<(), TaskError> {
        fs::create_dir_all(&self.root)?;
        self.persist_record(record)?;
        let worker_spec = TaskWorkerSpec {
            root: self.root.clone(),
            record: record.clone(),
            runner: CodexExecutionSpec {
                executable: self.codex_executable.clone(),
                prompt: record.task.prompt.clone(),
                workspace_root: record.execution.workspace_root.clone(),
                mode: record.execution.mode,
            },
        };
        let spec_path = self.spec_path(&record.execution.id);
        atomic_json_write(&spec_path, &worker_spec)?;
        let current_executable = env::current_exe()?;
        let mut command = Command::new(current_executable);
        command
            .arg("--task-worker")
            .arg(&spec_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_detached_process(&mut command);
        command.spawn().map_err(|error| {
            TaskError::new(format!("Task worker を起動できませんでした: {error}"))
        })?;
        Ok(())
    }

    fn control(&self, execution_id: &ExecutionId, control: &TaskControl) -> Result<(), TaskError> {
        atomic_json_write(&self.control_path(execution_id), control)
    }
}

pub fn run_task_worker(spec_path: &Path) -> Result<(), TaskError> {
    let bytes = fs::read(spec_path)?;
    let spec: TaskWorkerSpec = serde_json::from_slice(&bytes)?;
    let backend = LocalTaskBackend::new(spec.root, spec.runner.executable.clone());
    let mut record = spec.record;
    let execution_id = record.execution.id.clone();
    let (control_tx, control_rx) = mpsc::channel();
    let finished = Arc::new(AtomicBool::new(false));
    let poll_finished = finished.clone();
    let poll_backend = backend.clone();
    let poll_execution = execution_id.clone();
    let poller = thread::spawn(move || {
        while !poll_finished.load(Ordering::Relaxed) {
            match poll_backend.take_control(&poll_execution) {
                Ok(Some(control)) => {
                    if control_tx.send(control).is_err() {
                        break;
                    }
                }
                Ok(None) => {}
                Err(_) => {}
            }
            thread::sleep(Duration::from_millis(100));
        }
    });

    let mut last_persist = Instant::now();
    let run_result = run_codex_app_server(&spec.runner, control_rx, |update| {
        let urgent = match update {
            RunnerUpdate::Status(status) => {
                record.set_status(status);
                true
            }
            RunnerUpdate::Event(event) => {
                let urgent = matches!(
                    event,
                    TaskEvent::InputRequested { .. }
                        | TaskEvent::ApprovalRequested { .. }
                        | TaskEvent::Error { .. }
                );
                record.push_event(event);
                urgent
            }
            RunnerUpdate::ExternalThread(id) => {
                record.execution.external_thread_id = Some(id);
                true
            }
            RunnerUpdate::Failure(message) => {
                record.execution.failure = Some(message);
                true
            }
        };
        if urgent || last_persist.elapsed() >= Duration::from_millis(75) {
            let _ = backend.persist_record(&record);
            last_persist = Instant::now();
        }
    });
    finished.store(true, Ordering::Relaxed);
    let _ = poller.join();
    if let Err(error) = &run_result {
        record.execution.failure = Some(error.to_string());
        record.push_event(TaskEvent::Error {
            message: error.to_string(),
        });
        record.set_status(ExecutionStatus::Failed);
        backend.persist_record(&record)?;
    }
    let _ = fs::remove_file(spec_path);
    let _ = fs::remove_file(backend.control_path(&execution_id));
    run_result
}

fn atomic_json_write(path: &Path, value: &impl Serialize) -> Result<(), TaskError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(&mut temporary, value)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| TaskError::new(error.error.to_string()))?;
    Ok(())
}

fn default_codex_executable() -> PathBuf {
    if cfg!(windows)
        && let Some(app_data) = env::var_os("APPDATA")
    {
        let candidate = PathBuf::from(app_data).join("npm").join("codex.cmd");
        if candidate.is_file() {
            return candidate;
        }
    }
    PathBuf::from(if cfg!(windows) { "codex.cmd" } else { "codex" })
}

#[cfg(windows)]
fn configure_detached_process(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn configure_detached_process(_command: &mut Command) {}

#[derive(Clone)]
pub struct LocalGitBackend {
    state_root: PathBuf,
    worktree_root: PathBuf,
}

impl LocalGitBackend {
    pub fn new(state_root: PathBuf, worktree_root: PathBuf) -> Self {
        Self {
            state_root,
            worktree_root,
        }
    }

    pub fn user_default() -> Self {
        let base = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir)
            .join("Lapis");
        Self::new(base.join("git-v1"), base.join("worktrees"))
    }

    fn record_path(&self, task_id: &lapis_editor_core::TaskId) -> PathBuf {
        self.state_root
            .join(format!("worktree-{}.json", task_id.as_str()))
    }

    fn save_worktree(&self, worktree: &TaskWorktree) -> Result<(), GitError> {
        atomic_json_write(&self.record_path(&worktree.task_id), worktree)
            .map_err(|error| GitError::new(GitErrorKind::Io, error.to_string()))
    }
}

impl GitBackend for LocalGitBackend {
    fn status(&self, repository: &Path) -> Result<RepositoryStatus, GitError> {
        let root = git_text(repository, &["rev-parse", "--show-toplevel"])?;
        let head = git_text(repository, &["rev-parse", "HEAD"])?;
        let output = git_bytes(repository, &["status", "--porcelain=v1", "-z", "--branch"])?;
        let fields = output
            .split(|byte| *byte == 0)
            .filter(|field| !field.is_empty())
            .collect::<Vec<_>>();
        let mut branch = "HEAD".to_owned();
        let mut files = Vec::new();
        let mut index = 0;
        while index < fields.len() {
            let field = fields[index];
            let text = String::from_utf8_lossy(field);
            if let Some(value) = text.strip_prefix("## ") {
                branch = value.split("...").next().unwrap_or(value).to_owned();
                index += 1;
                continue;
            }
            if field.len() < 4 {
                return Err(GitError::new(
                    GitErrorKind::InvalidOutput,
                    "Git status の形式が不正です",
                ));
            }
            let x = char::from(field[0]);
            let y = char::from(field[1]);
            let path = PathBuf::from(String::from_utf8_lossy(&field[3..]).into_owned());
            let renamed_or_copied = matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C');
            let old_path = if renamed_or_copied {
                index += 1;
                fields
                    .get(index)
                    .map(|old| PathBuf::from(String::from_utf8_lossy(old).into_owned()))
            } else {
                None
            };
            let kind = if x == 'U' || y == 'U' || (x == 'A' && y == 'A') || (x == 'D' && y == 'D') {
                ChangeKind::Conflicted
            } else if x == '?' && y == '?' {
                ChangeKind::Untracked
            } else if x == 'A' || y == 'A' {
                ChangeKind::Added
            } else if x == 'D' || y == 'D' {
                ChangeKind::Deleted
            } else if x == 'R' || y == 'R' {
                ChangeKind::Renamed
            } else {
                ChangeKind::Modified
            };
            files.push(ChangedFile {
                path,
                old_path,
                kind,
                staged: x != ' ' && x != '?',
            });
            index += 1;
        }
        Ok(RepositoryStatus {
            root: PathBuf::from(root.trim()),
            branch,
            head: head.trim().to_owned(),
            files,
        })
    }

    fn diff(&self, repository: &Path, path: &Path) -> Result<FileDiff, GitError> {
        let path_text = path.to_string_lossy();
        let untracked = self
            .status(repository)?
            .files
            .iter()
            .any(|file| file.path == path && file.kind == ChangeKind::Untracked);
        let output = if untracked {
            git_bytes_allow_exit_code(
                repository,
                &[
                    "diff",
                    "--no-index",
                    "--no-ext-diff",
                    "--no-color",
                    "--",
                    "/dev/null",
                    &path_text,
                ],
                1,
            )?
        } else {
            git_bytes(
                repository,
                &[
                    "diff",
                    "HEAD",
                    "--no-ext-diff",
                    "--no-color",
                    "--",
                    &path_text,
                ],
            )?
        };
        let patch = String::from_utf8_lossy(&output).into_owned();
        let additions = patch
            .lines()
            .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
            .count();
        let deletions = patch
            .lines()
            .filter(|line| line.starts_with('-') && !line.starts_with("---"))
            .count();
        Ok(FileDiff {
            path: path.to_owned(),
            additions,
            deletions,
            binary: patch.contains("Binary files"),
            patch,
        })
    }

    fn create_worktree(
        &self,
        repository: &Path,
        task_id: &lapis_editor_core::TaskId,
    ) -> Result<TaskWorktree, GitError> {
        let status = self.status(repository)?;
        let repository_key = format!("{:016x}", stable_path_hash(&status.root));
        let path = self
            .worktree_root
            .join(repository_key)
            .join(task_id.as_str());
        if path.exists() {
            return Err(GitError::new(
                GitErrorKind::Conflict,
                format!("Task worktree が既に存在します: {}", path.display()),
            ));
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| GitError::new(GitErrorKind::Io, error.to_string()))?;
        }
        let path_text = path.to_string_lossy();
        git_bytes(
            &status.root,
            &["worktree", "add", "--detach", &path_text, &status.head],
        )?;
        let record = TaskWorktree {
            task_id: task_id.clone(),
            repository_root: status.root,
            path,
            base_commit: status.head,
            state: WorktreeState::Active,
            conflict: None,
        };
        self.save_worktree(&record)?;
        Ok(record)
    }

    fn worktrees(&self, repository: &Path) -> Result<Vec<TaskWorktree>, GitError> {
        let canonical = self.status(repository)?.root;
        let entries = match fs::read_dir(&self.state_root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(GitError::new(GitErrorKind::Io, error.to_string())),
        };
        let mut records = Vec::new();
        for entry in entries {
            let entry =
                entry.map_err(|error| GitError::new(GitErrorKind::Io, error.to_string()))?;
            if !entry.file_name().to_string_lossy().starts_with("worktree-") {
                continue;
            }
            let record: TaskWorktree = serde_json::from_slice(
                &fs::read(entry.path())
                    .map_err(|error| GitError::new(GitErrorKind::Io, error.to_string()))?,
            )
            .map_err(|error| GitError::new(GitErrorKind::InvalidOutput, error.to_string()))?;
            if record.repository_root == canonical {
                records.push(record);
            }
        }
        Ok(records)
    }

    fn import_file(&self, worktree: &TaskWorktree, path: &Path) -> Result<TaskWorktree, GitError> {
        if path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err(GitError::new(
                GitErrorKind::Conflict,
                "取込 path が Workspace 外です",
            ));
        }
        let path_text = path.to_string_lossy();
        let shared_status = git_bytes(
            &worktree.repository_root,
            &["status", "--porcelain=v1", "-z", "--", &path_text],
        )?;
        if !shared_status.is_empty() {
            let mut conflicted = worktree.clone();
            conflicted.state = WorktreeState::Conflict;
            conflicted.conflict = Some(format!(
                "共有Workspaceにも変更があります: {}",
                path.display()
            ));
            self.save_worktree(&conflicted)?;
            return Err(GitError::new(
                GitErrorKind::Conflict,
                conflicted.conflict.clone().unwrap_or_default(),
            ));
        }
        let source = worktree.path.join(path);
        let destination = worktree.repository_root.join(path);
        if source.is_file() {
            let bytes = fs::read(&source)
                .map_err(|error| GitError::new(GitErrorKind::Io, error.to_string()))?;
            let parent = destination.parent().unwrap_or_else(|| Path::new("."));
            fs::create_dir_all(parent)
                .map_err(|error| GitError::new(GitErrorKind::Io, error.to_string()))?;
            let mut temporary = tempfile::NamedTempFile::new_in(parent)
                .map_err(|error| GitError::new(GitErrorKind::Io, error.to_string()))?;
            temporary
                .write_all(&bytes)
                .and_then(|()| temporary.as_file().sync_all())
                .map_err(|error| GitError::new(GitErrorKind::Io, error.to_string()))?;
            temporary
                .persist(&destination)
                .map_err(|error| GitError::new(GitErrorKind::Io, error.error.to_string()))?;
        } else if destination.exists() {
            fs::remove_file(&destination)
                .map_err(|error| GitError::new(GitErrorKind::Io, error.to_string()))?;
        }
        let mut integrated = worktree.clone();
        integrated.state = WorktreeState::Integrated;
        integrated.conflict = None;
        self.save_worktree(&integrated)?;
        Ok(integrated)
    }

    fn discard_worktree(&self, worktree: &TaskWorktree) -> Result<TaskWorktree, GitError> {
        let path_text = worktree.path.to_string_lossy();
        git_bytes(
            &worktree.repository_root,
            &["worktree", "remove", "--force", &path_text],
        )?;
        let mut discarded = worktree.clone();
        discarded.state = WorktreeState::Discarded;
        discarded.conflict = None;
        self.save_worktree(&discarded)?;
        Ok(discarded)
    }
}

fn git_bytes(repository: &Path, arguments: &[&str]) -> Result<Vec<u8>, GitError> {
    git_bytes_allow_exit_code(repository, arguments, 0)
}

fn git_bytes_allow_exit_code(
    repository: &Path,
    arguments: &[&str],
    allowed_failure_code: i32,
) -> Result<Vec<u8>, GitError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(arguments)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|error| GitError::new(GitErrorKind::Io, error.to_string()))?;
    if output.status.success() || output.status.code() == Some(allowed_failure_code) {
        Ok(output.stdout)
    } else {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let kind = if message.contains("not a git repository") {
            GitErrorKind::NotRepository
        } else {
            GitErrorKind::Io
        };
        Err(GitError::new(kind, message))
    }
}

fn git_text(repository: &Path, arguments: &[&str]) -> Result<String, GitError> {
    Ok(String::from_utf8_lossy(&git_bytes(repository, arguments)?).into_owned())
}

fn stable_path_hash(path: &Path) -> u64 {
    path.to_string_lossy()
        .bytes()
        .fold(0xcbf29ce484222325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        })
}

fn format_snapshot(snapshot: &WorkspaceSnapshot) -> String {
    let mut lines = vec!["LAPIS_SESSION_V1".to_owned()];
    if let Some(root) = &snapshot.root {
        lines.push(format!(
            "root\t{}",
            encode_bytes(root.to_string_lossy().as_bytes())
        ));
    }
    if let Some(active) = &snapshot.active_path {
        lines.push(format!(
            "active\t{}",
            encode_bytes(active.to_string_lossy().as_bytes())
        ));
    }
    for view in &snapshot.open_documents {
        lines.push(format!(
            "doc\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            encode_bytes(view.path.to_string_lossy().as_bytes()),
            view.cursor_char,
            view.selection_start,
            view.selection_end,
            view.scroll_x,
            view.scroll_y,
            view.draft_content
                .as_deref()
                .map(|draft| encode_bytes(draft.as_bytes()))
                .unwrap_or_else(|| "-".to_owned())
        ));
    }
    lines.push(String::new());
    lines.join("\n")
}

fn parse_snapshot(text: &str) -> Result<WorkspaceSnapshot, WorkspaceError> {
    let mut lines = text.lines();
    if lines.next() != Some("LAPIS_SESSION_V1") {
        return Err(WorkspaceError::new("未対応のWorkspace復元形式です"));
    }
    let mut snapshot = WorkspaceSnapshot::default();
    for line in lines {
        let fields = line.split('\t').collect::<Vec<_>>();
        match fields.as_slice() {
            ["root", value] => snapshot.root = Some(decode_path(value)?),
            ["active", value] => snapshot.active_path = Some(decode_path(value)?),
            ["doc", path, cursor, start, end, scroll_x, scroll_y, draft] => {
                snapshot.open_documents.push(DocumentViewState {
                    document_id: None,
                    path: decode_path(path)?,
                    cursor_char: parse_number(cursor, "cursor")?,
                    selection_start: parse_number(start, "selection start")?,
                    selection_end: parse_number(end, "selection end")?,
                    scroll_x: parse_number(scroll_x, "scroll x")?,
                    scroll_y: parse_number(scroll_y, "scroll y")?,
                    draft_content: if *draft == "-" {
                        None
                    } else {
                        Some(
                            String::from_utf8(decode_bytes(draft)?)
                                .map_err(|error| WorkspaceError::new(error.to_string()))?,
                        )
                    },
                });
            }
            [] | [""] => {}
            _ => return Err(WorkspaceError::new("Workspace復元データが壊れています")),
        }
    }
    Ok(snapshot)
}

fn parse_number<T: std::str::FromStr>(value: &str, label: &str) -> Result<T, WorkspaceError> {
    value
        .parse()
        .map_err(|_| WorkspaceError::new(format!("{label}を復元できません")))
}

fn decode_path(value: &str) -> Result<PathBuf, WorkspaceError> {
    let bytes = decode_bytes(value)?;
    String::from_utf8(bytes)
        .map(PathBuf::from)
        .map_err(|error| WorkspaceError::new(error.to_string()))
}

fn encode_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn decode_bytes(value: &str) -> Result<Vec<u8>, WorkspaceError> {
    if !value.len().is_multiple_of(2) {
        return Err(WorkspaceError::new("16進復元データの長さが不正です"));
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = decode_hex(pair[0])?;
            let low = decode_hex(pair[1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn decode_hex(value: u8) -> Result<u8, WorkspaceError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(WorkspaceError::new("16進復元データが不正です")),
    }
}

impl<T: TaskBackend> TaskBackend for LoopbackBackend<T> {
    fn load(&self) -> Result<Vec<TaskRecord>, TaskError> {
        if !self.gate.is_connected() {
            return Err(TaskError::new("loopback workspace is disconnected"));
        }
        self.inner.load()
    }

    fn start(&self, record: &TaskRecord) -> Result<(), TaskError> {
        if !self.gate.is_connected() {
            return Err(TaskError::new("loopback workspace is disconnected"));
        }
        self.inner.start(record)
    }

    fn control(&self, execution_id: &ExecutionId, control: &TaskControl) -> Result<(), TaskError> {
        if !self.gate.is_connected() {
            return Err(TaskError::new("loopback workspace is disconnected"));
        }
        self.inner.control(execution_id, control)
    }
}

impl<T: GitBackend> GitBackend for LoopbackBackend<T> {
    fn status(&self, repository: &Path) -> Result<RepositoryStatus, GitError> {
        if !self.gate.is_connected() {
            return Err(GitError::new(
                GitErrorKind::Io,
                "loopback workspace is disconnected",
            ));
        }
        self.inner.status(repository)
    }

    fn diff(&self, repository: &Path, path: &Path) -> Result<FileDiff, GitError> {
        if !self.gate.is_connected() {
            return Err(GitError::new(
                GitErrorKind::Io,
                "loopback workspace is disconnected",
            ));
        }
        self.inner.diff(repository, path)
    }

    fn create_worktree(
        &self,
        repository: &Path,
        task_id: &lapis_editor_core::TaskId,
    ) -> Result<TaskWorktree, GitError> {
        if !self.gate.is_connected() {
            return Err(GitError::new(
                GitErrorKind::Io,
                "loopback workspace is disconnected",
            ));
        }
        self.inner.create_worktree(repository, task_id)
    }

    fn worktrees(&self, repository: &Path) -> Result<Vec<TaskWorktree>, GitError> {
        if !self.gate.is_connected() {
            return Err(GitError::new(
                GitErrorKind::Io,
                "loopback workspace is disconnected",
            ));
        }
        self.inner.worktrees(repository)
    }

    fn import_file(&self, worktree: &TaskWorktree, path: &Path) -> Result<TaskWorktree, GitError> {
        if !self.gate.is_connected() {
            return Err(GitError::new(
                GitErrorKind::Io,
                "loopback workspace is disconnected",
            ));
        }
        self.inner.import_file(worktree, path)
    }

    fn discard_worktree(&self, worktree: &TaskWorktree) -> Result<TaskWorktree, GitError> {
        if !self.gate.is_connected() {
            return Err(GitError::new(
                GitErrorKind::Io,
                "loopback workspace is disconnected",
            ));
        }
        self.inner.discard_worktree(worktree)
    }
}

impl<T: TerminalBackend> TerminalBackend for LoopbackBackend<T> {
    fn start(&self, cwd: &Path, columns: u16, rows: u16) -> Result<TerminalId, TerminalError> {
        if !self.gate.is_connected() {
            return Err(TerminalError::new("loopback workspace is disconnected"));
        }
        self.inner.start(cwd, columns, rows)
    }

    fn input(&self, id: &TerminalId, bytes: &[u8]) -> Result<(), TerminalError> {
        if !self.gate.is_connected() {
            return Err(TerminalError::new("loopback workspace is disconnected"));
        }
        self.inner.input(id, bytes)
    }

    fn resize(&self, id: &TerminalId, columns: u16, rows: u16) -> Result<(), TerminalError> {
        if !self.gate.is_connected() {
            return Err(TerminalError::new("loopback workspace is disconnected"));
        }
        self.inner.resize(id, columns, rows)
    }

    fn poll(&self, id: &TerminalId) -> Result<Vec<TerminalEvent>, TerminalError> {
        if !self.gate.is_connected() {
            return Err(TerminalError::new("loopback workspace is disconnected"));
        }
        self.inner.poll(id)
    }

    fn terminate(&self, id: &TerminalId) -> Result<(), TerminalError> {
        if !self.gate.is_connected() {
            return Err(TerminalError::new("loopback workspace is disconnected"));
        }
        self.inner.terminate(id)
    }
}

impl<T: LanguageServerBackend> LanguageServerBackend for LoopbackBackend<T> {
    fn start(&self, workspace: &Path) -> Result<(), LspError> {
        if !self.gate.is_connected() {
            return Err(LspError::new("loopback workspace is disconnected"));
        }
        self.inner.start(workspace)
    }
    fn did_open(
        &self,
        path: &Path,
        text: &str,
        revision: lapis_document::Revision,
    ) -> Result<(), LspError> {
        if !self.gate.is_connected() {
            return Err(LspError::new("loopback workspace is disconnected"));
        }
        self.inner.did_open(path, text, revision)
    }
    fn did_change(
        &self,
        path: &Path,
        text: &str,
        revision: lapis_document::Revision,
    ) -> Result<(), LspError> {
        if !self.gate.is_connected() {
            return Err(LspError::new("loopback workspace is disconnected"));
        }
        self.inner.did_change(path, text, revision)
    }
    fn diagnostics(&self) -> Result<Vec<Diagnostic>, LspError> {
        if !self.gate.is_connected() {
            return Err(LspError::new("loopback workspace is disconnected"));
        }
        self.inner.diagnostics()
    }
    fn completion(
        &self,
        path: &Path,
        position: LspPosition,
        revision: lapis_document::Revision,
    ) -> Result<Vec<CompletionItem>, LspError> {
        if !self.gate.is_connected() {
            return Err(LspError::new("loopback workspace is disconnected"));
        }
        self.inner.completion(path, position, revision)
    }
    fn definition(
        &self,
        path: &Path,
        position: LspPosition,
        revision: lapis_document::Revision,
    ) -> Result<Option<DefinitionTarget>, LspError> {
        if !self.gate.is_connected() {
            return Err(LspError::new("loopback workspace is disconnected"));
        }
        self.inner.definition(path, position, revision)
    }
    fn shutdown(&self) -> Result<(), LspError> {
        if !self.gate.is_connected() {
            return Err(LspError::new("loopback workspace is disconnected"));
        }
        self.inner.shutdown()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static RUST_ANALYZER_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn rust_analyzer_workspace() -> (tempfile::TempDir, PathBuf) {
        let workspace = tempfile::tempdir().unwrap();
        fs::create_dir(workspace.path().join("src")).unwrap();
        fs::write(
            workspace.path().join("Cargo.toml"),
            "[package]\nname = \"lapis-lsp-test\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        let path = workspace.path().join("src/lib.rs");
        fs::write(&path, "").unwrap();
        (workspace, path)
    }

    #[test]
    fn local_repository_round_trips_and_detects_conflicts() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        let repository = LocalWorkspaceRepository;

        let saved = repository.write_file(&path, b"# Hello\n", None).unwrap();
        assert_eq!(repository.read_file(&path).unwrap().bytes, b"# Hello\n");

        fs::write(&path, b"external").unwrap();
        assert!(
            repository
                .write_file(&path, b"local", Some(&saved))
                .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), b"external");
    }

    #[test]
    fn file_tree_uses_real_paths_and_skips_internal_directories() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir_all(directory.path().join("src")).unwrap();
        fs::create_dir_all(directory.path().join("target")).unwrap();
        fs::write(directory.path().join("src").join("main.rs"), "fn main() {}").unwrap();
        fs::write(directory.path().join("target").join("hidden"), "x").unwrap();

        let tree = LocalWorkspaceRepository
            .list_tree(directory.path())
            .unwrap();
        assert!(
            tree.iter()
                .any(|entry| entry.relative_path == Path::new("src/main.rs"))
        );
        assert!(
            !tree
                .iter()
                .any(|entry| entry.relative_path.starts_with("target"))
        );
    }

    #[test]
    fn snapshot_round_trip_preserves_unicode_draft_and_view() {
        let directory = tempfile::tempdir().unwrap();
        let state = LocalWorkspaceStateRepository::new(directory.path().join("session.txt"));
        let snapshot = WorkspaceSnapshot {
            root: Some(PathBuf::from("C:/作業/lapis")),
            active_path: Some(PathBuf::from("C:/作業/lapis/メモ.md")),
            open_documents: vec![DocumentViewState {
                path: PathBuf::from("C:/作業/lapis/メモ.md"),
                cursor_char: 4,
                selection_start: 1,
                selection_end: 4,
                scroll_x: 12.5,
                scroll_y: 33.0,
                draft_content: Some("日本😀\n".to_owned()),
                ..DocumentViewState::default()
            }],
        };
        state.save(&snapshot).unwrap();
        assert_eq!(state.load().unwrap(), Some(snapshot));
    }

    #[test]
    fn git_worktree_diff_import_conflict_and_discard() {
        let directory = tempfile::tempdir().unwrap();
        let repository = directory.path().join("repo");
        fs::create_dir_all(&repository).unwrap();
        let git = |args: &[&str]| {
            let status = Command::new("git")
                .arg("-C")
                .arg(&repository)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "lapis@example.invalid"]);
        git(&["config", "user.name", "Lapis Test"]);
        fs::write(repository.join("note.txt"), "base\n").unwrap();
        git(&["add", "note.txt"]);
        git(&["commit", "-q", "-m", "base"]);

        let backend = LocalGitBackend::new(
            directory.path().join("state"),
            directory.path().join("worktrees"),
        );
        assert!(backend.status(&repository).unwrap().files.is_empty());
        let task = lapis_editor_core::TaskId::new("task-git-1");
        let worktree = backend.create_worktree(&repository, &task).unwrap();
        fs::write(worktree.path.join("note.txt"), "base\nworktree\n").unwrap();
        let status = backend.status(&worktree.path).unwrap();
        assert_eq!(status.files.len(), 1);
        let diff = backend.diff(&worktree.path, Path::new("note.txt")).unwrap();
        assert_eq!(diff.additions, 1);
        backend
            .import_file(&worktree, Path::new("note.txt"))
            .unwrap();
        assert_eq!(
            fs::read_to_string(repository.join("note.txt")).unwrap(),
            "base\nworktree\n"
        );

        let task2 = lapis_editor_core::TaskId::new("task-git-2");
        let worktree2 = backend.create_worktree(&repository, &task2).unwrap();
        fs::write(worktree2.path.join("note.txt"), "other\n").unwrap();
        let conflict = backend
            .import_file(&worktree2, Path::new("note.txt"))
            .unwrap_err();
        assert_eq!(conflict.kind(), GitErrorKind::Conflict);

        assert_eq!(
            backend.discard_worktree(&worktree).unwrap().state,
            WorktreeState::Discarded
        );
        assert_eq!(
            backend.discard_worktree(&worktree2).unwrap().state,
            WorktreeState::Discarded
        );
        assert!(!worktree.path.exists());
        assert!(!worktree2.path.exists());
    }

    #[test]
    fn git_status_keeps_rename_source_and_untracked_diff() {
        let directory = tempfile::tempdir().unwrap();
        let repository = directory.path().join("repo");
        fs::create_dir_all(&repository).unwrap();
        let git = |args: &[&str]| {
            let status = Command::new("git")
                .arg("-C")
                .arg(&repository)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "lapis@example.invalid"]);
        git(&["config", "user.name", "Lapis Test"]);
        fs::write(repository.join("old.txt"), "base\n").unwrap();
        git(&["add", "old.txt"]);
        git(&["commit", "-q", "-m", "base"]);

        fs::rename(repository.join("old.txt"), repository.join("renamed.txt")).unwrap();
        git(&["add", "-A"]);
        let backend = LocalGitBackend::new(
            directory.path().join("state"),
            directory.path().join("worktrees"),
        );
        let status = backend.status(&repository).unwrap();
        assert_eq!(status.files.len(), 1);
        assert_eq!(status.files[0].kind, ChangeKind::Renamed);
        assert_eq!(status.files[0].path, Path::new("renamed.txt"));
        assert_eq!(
            status.files[0].old_path.as_deref(),
            Some(Path::new("old.txt"))
        );

        fs::write(repository.join("new.txt"), "new file\n").unwrap();
        let diff = backend.diff(&repository, Path::new("new.txt")).unwrap();
        assert_eq!(diff.additions, 1);
        assert!(diff.patch.contains("new file"));
    }

    #[test]
    fn terminal_pty_accepts_input_and_returns_output() {
        let backend = LocalTerminalBackend::default();
        let id = backend.start(Path::new("."), 100, 30).unwrap();
        backend.resize(&id, 120, 32).unwrap();
        backend
            .input(&id, b"Write-Output lapis-terminal-smoke\r")
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut output = String::new();
        while Instant::now() < deadline {
            for event in backend.poll(&id).unwrap() {
                if let TerminalEvent::Output(text) = event {
                    output.push_str(&text);
                }
            }
            if output.contains("lapis-terminal-smoke") {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        backend.terminate(&id).unwrap();
        assert!(output.contains("lapis-terminal-smoke"), "output: {output}");
    }

    #[test]
    fn rust_analyzer_reports_virtual_document_diagnostics_and_stops() {
        let _guard = RUST_ANALYZER_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let (workspace, path) = rust_analyzer_workspace();
        let backend = LocalLspBackend::default();
        if let Err(error) = backend.start(workspace.path()) {
            assert!(error.to_string().contains("rust-analyzer is unavailable"));
            return;
        }
        backend
            .did_open(&path, "fn broken( {\n", lapis_document::Revision::default())
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut diagnostics = Vec::new();
        while Instant::now() < deadline {
            diagnostics = backend.diagnostics().unwrap();
            if !diagnostics.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        backend.shutdown().unwrap();
        assert!(!diagnostics.is_empty());
        let expected_path = normalize_lsp_path(&path);
        assert!(diagnostics.iter().any(|item| {
            normalize_lsp_path(&item.path) == expected_path
                && item.severity == DiagnosticSeverity::Error
        }));
    }

    #[test]
    fn rust_analyzer_completes_and_finds_virtual_document_definition() {
        let _guard = RUST_ANALYZER_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let (workspace, path) = rust_analyzer_workspace();
        let text = concat!(
            "pub fn lapis_helper() -> u32 { 1 }\n",
            "pub fn caller() -> u32 { lapis_helper() }\n",
            "pub fn completion() { let value = String::new(); value. }\n",
        );
        let backend = LocalLspBackend::default();
        backend.start(workspace.path()).unwrap();
        let revision = lapis_document::Revision::default();
        backend.did_open(&path, text, revision).unwrap();
        let _ = backend.diagnostics().unwrap();

        let completion_line = text.lines().nth(2).unwrap();
        let completion_column = completion_line.find("value.").unwrap() + "value.".len();
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut completions = Vec::new();
        while Instant::now() < deadline {
            completions = backend
                .completion(
                    &path,
                    LspPosition {
                        line: 2,
                        utf16_column: completion_column as u32,
                    },
                    revision,
                )
                .unwrap();
            if !completions.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        assert!(
            completions.iter().any(|item| item.label == "len"),
            "completion labels: {:?}",
            completions
                .iter()
                .take(20)
                .map(|item| &item.label)
                .collect::<Vec<_>>()
        );

        let definition_line = text.lines().nth(1).unwrap();
        let definition_column = definition_line.find("lapis_helper").unwrap() + 2;
        let definition = backend
            .definition(
                &path,
                LspPosition {
                    line: 1,
                    utf16_column: definition_column as u32,
                },
                revision,
            )
            .unwrap()
            .expect("lapis_helper definition");
        backend.shutdown().unwrap();
        assert_eq!(definition.path, normalize_lsp_path(&path));
        assert_eq!(definition.range.start.line, 0);
    }

    #[test]
    fn workspace_search_finds_utf8_text_and_skips_build_output() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir_all(directory.path().join("src")).unwrap();
        fs::create_dir_all(directory.path().join("target")).unwrap();
        fs::write(
            directory.path().join("src/main.rs"),
            "fn main() { /* 検索語 */ }\n",
        )
        .unwrap();
        fs::write(directory.path().join("target/generated.rs"), "検索語\n").unwrap();
        let hits = LocalWorkspaceSearchBackend
            .search(directory.path(), "検索語", &AtomicBool::new(false))
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, Path::new("src/main.rs"));
        assert_eq!(hits[0].line, 1);
    }

    #[test]
    fn loopback_disconnects_and_reconnects_without_losing_workspace_data() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("note.md");
        fs::write(&path, "kept across reconnect\n").unwrap();
        let gate = ConnectionGate::connected();
        let backend = LoopbackBackend::new(Arc::new(LocalWorkspaceRepository), gate.clone());

        assert!(backend.read_file(&path).is_ok());
        gate.disconnect();
        assert!(backend.read_file(&path).is_err());
        assert!(backend.list_tree(directory.path()).is_err());
        gate.connect();

        let restored = backend.read_file(&path).unwrap();
        assert_eq!(restored.bytes, b"kept across reconnect\n");
        assert!(backend.list_tree(directory.path()).is_ok());
    }
}
