//! Local Host の rust-analyzer を使う Language Server backend。

use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex, MutexGuard, mpsc},
    thread,
    time::{Duration, Instant},
};

use futures_channel::mpsc::UnboundedSender;
use lsp::{
    CompletionItem, DefinitionTarget, Diagnostic, DiagnosticSeverity, DiagnosticsChanges,
    LanguageServerBackend, LspError, LspPosition, LspRange,
};

struct LspState {
    writer: Box<dyn Write + Send>,
    revisions: HashMap<PathBuf, document::Revision>,
    diagnostics: Vec<Diagnostic>,
    diagnostics_dirty: bool,
}

struct LspProcess {
    child: std::process::Child,
    state: Arc<Mutex<LspState>>,
    responses: mpsc::Receiver<serde_json::Value>,
    next_request: u64,
}

#[derive(Default)]
pub struct LocalLspBackend {
    process: Mutex<Option<LspProcess>>,
    diagnostics_notifier: Arc<Mutex<Option<UnboundedSender<()>>>>,
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

fn lock_state(state: &Mutex<LspState>) -> Result<MutexGuard<'_, LspState>, LspError> {
    state
        .lock()
        .map_err(|_| LspError::new("Language server state lock failed"))
}

fn notify_diagnostics_changed(notifier: &Mutex<Option<UnboundedSender<()>>>) {
    if let Ok(guard) = notifier.lock()
        && let Some(sender) = guard.as_ref()
    {
        let _ = sender.unbounded_send(());
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
        let (sender, responses) = mpsc::channel();
        let state = Arc::new(Mutex::new(LspState {
            writer: Box::new(writer),
            revisions: HashMap::new(),
            diagnostics: Vec::new(),
            diagnostics_dirty: false,
        }));
        let reader_state = state.clone();
        let notifier = self.diagnostics_notifier.clone();
        thread::spawn(move || read_lsp_messages(stdout, sender, reader_state, notifier));
        let mut process = LspProcess {
            child,
            state,
            responses,
            next_request: 1,
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
        revision: document::Revision,
    ) -> Result<(), LspError> {
        self.with_process(|process| {
            let path = normalize_lsp_path(path);
            let mut state = lock_state(&process.state)?;
            state.revisions.insert(path.clone(), revision);
            state.diagnostics_dirty = true;
            lsp_write(
                state.writer.as_mut(),
                &serde_json::json!({
                    "jsonrpc": "2.0", "method": "textDocument/didOpen",
                    "params": { "textDocument": {
                        "uri": file_uri(&path), "languageId": "rust",
                        "version": revision.number(), "text": text
                    }}
                }),
            )
        })
    }

    fn did_change(
        &self,
        path: &Path,
        text: &str,
        revision: document::Revision,
    ) -> Result<(), LspError> {
        self.with_process(|process| {
            let path = normalize_lsp_path(path);
            let mut state = lock_state(&process.state)?;
            state.revisions.insert(path.clone(), revision);
            state.diagnostics_dirty = true;
            lsp_write(
                state.writer.as_mut(),
                &serde_json::json!({
                    "jsonrpc": "2.0", "method": "textDocument/didChange",
                    "params": {
                        "textDocument": { "uri": file_uri(&path), "version": revision.number() },
                        "contentChanges": [{ "text": text }]
                    }
                }),
            )
        })
    }

    fn diagnostics(&self) -> Result<Vec<Diagnostic>, LspError> {
        self.with_process(|process| {
            let documents = {
                let mut state = lock_state(&process.state)?;
                if !state.diagnostics_dirty {
                    return Ok(state.diagnostics.clone());
                }
                state.diagnostics_dirty = false;
                state
                    .revisions
                    .iter()
                    .map(|(path, revision)| (path.clone(), *revision))
                    .collect::<Vec<_>>()
            };
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
                let mut state = lock_state(&process.state)?;
                replace_diagnostics(&mut state.diagnostics, &path, revision, items);
            }
            let state = lock_state(&process.state)?;
            Ok(state.diagnostics.clone())
        })
    }

    fn diagnostics_changes(&self) -> Option<DiagnosticsChanges> {
        let mut notifier = self.diagnostics_notifier.lock().ok()?;
        if notifier.is_some() {
            return None;
        }
        let (sender, receiver) = futures_channel::mpsc::unbounded();
        *notifier = Some(sender);
        Some(receiver)
    }

    fn completion(
        &self,
        path: &Path,
        position: LspPosition,
        revision: document::Revision,
    ) -> Result<Vec<CompletionItem>, LspError> {
        self.with_process(|process| {
            let path = normalize_lsp_path(path);
            if lock_state(&process.state)?.revisions.get(&path) != Some(&revision) {
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
        revision: document::Revision,
    ) -> Result<Option<DefinitionTarget>, LspError> {
        self.with_process(|process| {
            let path = normalize_lsp_path(path);
            if lock_state(&process.state)?.revisions.get(&path) != Some(&revision) {
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

fn read_lsp_messages(
    stdout: std::process::ChildStdout,
    responses: mpsc::Sender<serde_json::Value>,
    state: Arc<Mutex<LspState>>,
    notifier: Arc<Mutex<Option<UnboundedSender<()>>>>,
) {
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
        let Ok(message) = serde_json::from_slice::<serde_json::Value>(&body) else {
            continue;
        };
        // 応答は待機中の要求へ渡し、通知と server request は読み取りスレッドで処理する。
        if message.get("id").is_some() && message.get("method").is_none() {
            if responses.send(message).is_err() {
                return;
            }
            continue;
        }
        lsp_handle_incoming(&state, &message, &notifier);
    }
}

fn lsp_write(writer: &mut (dyn Write + Send), value: &serde_json::Value) -> Result<(), LspError> {
    let bytes = serde_json::to_vec(value).map_err(|error| LspError::new(error.to_string()))?;
    write!(writer, "Content-Length: {}\r\n\r\n", bytes.len())
        .and_then(|()| writer.write_all(&bytes))
        .and_then(|()| writer.flush())
        .map_err(|error| LspError::new(error.to_string()))
}

fn lsp_request(
    process: &mut LspProcess,
    method: &str,
    params: serde_json::Value,
) -> Result<u64, LspError> {
    let id = process.next_request;
    process.next_request += 1;
    let mut state = lock_state(&process.state)?;
    lsp_write(
        state.writer.as_mut(),
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
    let mut state = lock_state(&process.state)?;
    lsp_write(
        state.writer.as_mut(),
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
            .responses
            .recv_timeout(remaining)
            .map_err(|error| LspError::new(format!("LSP response failed: {error}")))?;
        if message.get("id").and_then(|value| value.as_u64()) == Some(request_id) {
            return Ok(message);
        }
    }
}

fn lsp_handle_incoming(
    state: &Mutex<LspState>,
    message: &serde_json::Value,
    notifier: &Mutex<Option<UnboundedSender<()>>>,
) {
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
                if let Ok(mut state) = state.lock() {
                    state.diagnostics_dirty = true;
                }
                notify_diagnostics_changed(notifier);
                serde_json::Value::Null
            }
            _ => {
                if let Ok(mut state) = state.lock() {
                    let _ = lsp_write(
                        state.writer.as_mut(),
                        &serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": message.get("id").cloned().unwrap_or(serde_json::Value::Null),
                            "error": { "code": -32601, "message": format!("Unsupported server request: {method}") }
                        }),
                    );
                }
                return;
            }
        };
        if let Ok(mut state) = state.lock() {
            let _ = lsp_write(
                state.writer.as_mut(),
                &serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": message.get("id").cloned().unwrap_or(serde_json::Value::Null),
                    "result": result
                }),
            );
        }
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
    let Ok(mut state) = state.lock() else {
        return;
    };
    let Some(revision) = state.revisions.get(&path).copied() else {
        return;
    };
    let items = params
        .get("diagnostics")
        .and_then(|value| value.as_array())
        .map(Vec::as_slice);
    if replace_diagnostics(&mut state.diagnostics, &path, revision, items) {
        notify_diagnostics_changed(notifier);
    }
}

fn replace_diagnostics(
    diagnostics: &mut Vec<Diagnostic>,
    path: &Path,
    revision: document::Revision,
    items: Option<&[serde_json::Value]>,
) -> bool {
    let previous = diagnostics.clone();
    diagnostics.retain(|diagnostic| diagnostic.path != path);
    diagnostics.extend(items.into_iter().flatten().filter_map(|item| {
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
    *diagnostics != previous
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
    let value = uri.strip_prefix("file://").unwrap_or(uri);
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
    #[cfg(windows)]
    let decoded = {
        let bytes = decoded.as_bytes();
        let windows_drive_path =
            bytes.get(1).is_some_and(u8::is_ascii_alphabetic) && bytes.get(2) == Some(&b':');
        if decoded.starts_with(std::path::MAIN_SEPARATOR) && windows_drive_path {
            decoded[1..].to_owned()
        } else {
            decoded
        }
    };
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

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
    fn file_uri_round_trips_absolute_paths() {
        let path = if cfg!(windows) {
            PathBuf::from("C:/lapis/lsp test/メモ %.rs")
        } else {
            PathBuf::from("/tmp/lapis/lsp test/メモ %.rs")
        };

        assert_eq!(path_from_file_uri(&file_uri(&path)), path);
    }

    #[test]
    fn diagnostics_notifications_are_delivered_without_a_request() {
        let workspace = tempfile::tempdir().unwrap();
        fs::create_dir(workspace.path().join("src")).unwrap();
        let path = workspace.path().join("src/lib.rs");
        fs::write(&path, "").unwrap();
        let path = normalize_lsp_path(&path);
        let (sender, mut changes) = futures_channel::mpsc::unbounded();
        let notifier = Mutex::new(Some(sender));
        let state = Mutex::new(LspState {
            writer: Box::new(Vec::new()),
            revisions: HashMap::from([(path.clone(), document::Revision::default())]),
            diagnostics: Vec::new(),
            diagnostics_dirty: false,
        });

        lsp_handle_incoming(
            &state,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "method": "textDocument/publishDiagnostics",
                "params": {
                    "uri": file_uri(&path),
                    "diagnostics": [{
                        "range": {
                            "start": { "line": 0, "character": 0 },
                            "end": { "line": 0, "character": 1 }
                        },
                        "severity": 1,
                        "message": "broken"
                    }]
                }
            }),
            &notifier,
        );

        assert!(matches!(changes.try_recv(), Ok(())));
        assert_eq!(state.lock().unwrap().diagnostics.len(), 1);
        // 内容が変わらない再送では通知しない。
        lsp_handle_incoming(
            &state,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "method": "textDocument/publishDiagnostics",
                "params": {
                    "uri": file_uri(&path),
                    "diagnostics": [{
                        "range": {
                            "start": { "line": 0, "character": 0 },
                            "end": { "line": 0, "character": 1 }
                        },
                        "severity": 1,
                        "message": "broken"
                    }]
                }
            }),
            &notifier,
        );
        assert!(changes.try_recv().is_err());

        lsp_handle_incoming(
            &state,
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "workspace/diagnostic/refresh",
                "params": {}
            }),
            &notifier,
        );
        assert!(matches!(changes.try_recv(), Ok(())));
        assert!(state.lock().unwrap().diagnostics_dirty);
    }

    #[test]
    fn rust_analyzer_pushes_diagnostics_after_initial_analysis() {
        let _guard = RUST_ANALYZER_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let (workspace, path) = rust_analyzer_workspace();
        let backend = LocalLspBackend::default();
        if let Err(error) = backend.start(workspace.path()) {
            assert!(error.to_string().contains("rust-analyzer is unavailable"));
            return;
        }
        let mut changes = backend
            .diagnostics_changes()
            .expect("diagnostics changes subscription");
        backend
            .did_open(&path, "fn broken( {\n", document::Revision::default())
            .unwrap();
        // 初回の解析を開始させ、以降は通知だけで更新されることを確認する。
        let _ = backend.diagnostics().unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        let notified = loop {
            match changes.try_recv() {
                Ok(()) => break true,
                Err(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
                Err(_) => break false,
            }
        };
        backend.shutdown().unwrap();
        assert!(
            notified,
            "rust-analyzer の publishDiagnostics を要求なしで受信できませんでした"
        );
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
            .did_open(&path, "fn broken( {\n", document::Revision::default())
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
        let revision = document::Revision::default();
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
}
