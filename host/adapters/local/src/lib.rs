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
    time::{Duration, Instant},
};

use document::{DocumentError, DocumentRepository, FileData, FileFingerprint};
use editor_core::ExecutionId;
use git::{
    ChangeKind, ChangedFile, FileDiff, GitBackend, GitError, GitErrorKind, RepositoryStatus,
    TaskWorktree, WorktreeState,
};
use lsp::{
    CompletionItem, DefinitionTarget, Diagnostic, DiagnosticSeverity, LanguageServerBackend,
    LspError, LspPosition, LspRange,
};
use task_runner::{TaskBackend, TaskControl, TaskError, TaskRecord};
use terminal::{
    TerminalBackend, TerminalDelta, TerminalError, TerminalEvent, TerminalId, TerminalInput,
};
use workspace::WorkspaceRepository;
use workspace::{FileEntry, SearchError, SearchHit, WorkspaceError, WorkspaceSearchBackend};
mod git_action;
mod git_watch;
pub use local_credentials::{CredentialStore, CredentialStoreError};
pub use local_tasks::{LocalTaskBackend, run_task_worker};
pub use local_terminal::LocalTerminalBackend;
pub use local_workbench::LocalWorkbenchStore;
pub use local_workspace::{LocalWorkspaceRepository, LocalWorkspaceSearchBackend};

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

struct LspProcess {
    child: std::process::Child,
    writer: std::process::ChildStdin,
    receiver: mpsc::Receiver<serde_json::Value>,
    next_request: u64,
    revisions: HashMap<PathBuf, document::Revision>,
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
        revision: document::Revision,
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
        revision: document::Revision,
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
        revision: document::Revision,
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
        revision: document::Revision,
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
    revision: document::Revision,
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
    fn create_directory(&self, root: &Path, name: &str) -> Result<(), WorkspaceError> {
        if !self.gate.is_connected() {
            return Err(WorkspaceError::new("loopback workspace is disconnected"));
        }
        self.inner.create_directory(root, name)
    }
    fn list_tree(&self, root: &Path) -> Result<Vec<FileEntry>, WorkspaceError> {
        if !self.gate.is_connected() {
            return Err(WorkspaceError::new("loopback workspace is disconnected"));
        }
        self.inner.list_tree(root)
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

    fn record_path(&self, task_id: &editor_core::TaskId) -> PathBuf {
        self.state_root
            .join(format!("worktree-{}.json", task_id.as_str()))
    }

    fn save_worktree(&self, worktree: &TaskWorktree) -> Result<(), GitError> {
        local_fs::atomic_json_write(&self.record_path(&worktree.task_id), worktree)
            .map_err(|error| GitError::new(GitErrorKind::Io, error.to_string()))
    }
}

impl GitBackend for LocalGitBackend {
    fn watch_repository(
        &self,
        repository: &Path,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Box<dyn git::GitWatch>, GitError> {
        Ok(Box::new(git_watch::LocalGitWatch::new(repository, notify)?))
    }
    fn discover_repositories(&self, workspace: &Path) -> Result<Vec<PathBuf>, GitError> {
        let root = fs::canonicalize(workspace)
            .map_err(|error| GitError::new(GitErrorKind::Io, error.to_string()))?;
        let mut candidates = vec![root.clone()];
        for entry in fs::read_dir(&root)
            .map_err(|error| GitError::new(GitErrorKind::Io, error.to_string()))?
        {
            let entry =
                entry.map_err(|error| GitError::new(GitErrorKind::Io, error.to_string()))?;
            if entry
                .file_type()
                .map_err(|error| GitError::new(GitErrorKind::Io, error.to_string()))?
                .is_dir()
            {
                candidates.push(entry.path());
            }
        }
        let mut repositories = Vec::new();
        for candidate in candidates {
            match fs::metadata(candidate.join(".git")) {
                Ok(metadata) if metadata.is_dir() || metadata.is_file() => {}
                Ok(_) => continue,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(GitError::new(GitErrorKind::Io, error.to_string())),
            };
            let actual = match git_text(&candidate, &["rev-parse", "--show-toplevel"]) {
                Ok(actual) => actual,
                Err(error) if error.kind() == GitErrorKind::NotRepository => continue,
                Err(error) => return Err(error),
            };
            let actual = fs::canonicalize(actual.trim())
                .map_err(|error| GitError::new(GitErrorKind::Io, error.to_string()))?;
            if actual
                == fs::canonicalize(&candidate)
                    .map_err(|error| GitError::new(GitErrorKind::Io, error.to_string()))?
                && actual.starts_with(&root)
            {
                repositories.push(actual);
            }
        }
        repositories.sort();
        repositories.dedup();
        Ok(repositories)
    }
    fn execute(
        &self,
        repository: &Path,
        request: git::GitActionRequest,
    ) -> Result<git::GitActionResult, GitError> {
        let gate = git_repository_gate(repository)?;
        let _guard = gate
            .lock()
            .map_err(|_| GitError::new(GitErrorKind::Io, "Repositoryの操作lockが利用できません"))?;
        self.execute_action(repository, request)
    }
    fn status(&self, repository: &Path) -> Result<RepositoryStatus, GitError> {
        let root = git_text(repository, &["rev-parse", "--show-toplevel"])?;
        let head = git_text(repository, &["rev-parse", "--verify", "HEAD"]).unwrap_or_default();
        let output = git_bytes(
            repository,
            &[
                "status",
                "--porcelain=v1",
                "-z",
                "--branch",
                "--untracked-files=all",
            ],
        )?;
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
            if x == 'U' || y == 'U' || (x == 'A' && y == 'A') || (x == 'D' && y == 'D') {
                files.push(ChangedFile {
                    path,
                    old_path,
                    kind: ChangeKind::Conflicted,
                    staged: false,
                });
            } else if x == '?' && y == '?' {
                files.push(ChangedFile {
                    path,
                    old_path,
                    kind: ChangeKind::Untracked,
                    staged: false,
                });
            } else {
                for (change, staged) in [(x, true), (y, false)] {
                    if change == ' ' {
                        continue;
                    }
                    let kind = match change {
                        'A' => ChangeKind::Added,
                        'D' => ChangeKind::Deleted,
                        'R' | 'C' => ChangeKind::Renamed,
                        _ => ChangeKind::Modified,
                    };
                    files.push(ChangedFile {
                        path: path.clone(),
                        old_path: old_path.clone(),
                        kind,
                        staged,
                    });
                }
            }
            index += 1;
        }
        Ok(RepositoryStatus {
            root: PathBuf::from(root.trim()),
            branch: git_text(repository, &["symbolic-ref", "--quiet", "--short", "HEAD"])
                .map(|name| name.trim().to_owned())
                .unwrap_or(branch),
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
        task_id: &editor_core::TaskId,
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
        let common = git_common_directory(repository)?;
        let entries = match fs::read_dir(&self.state_root) {
            Ok(entries) => Some(entries),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(GitError::new(GitErrorKind::Io, error.to_string())),
        };
        let mut records = Vec::new();
        for entry in entries.into_iter().flatten() {
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
            records.push(record);
        }
        let listing = git_bytes(repository, &["worktree", "list", "--porcelain", "-z"])?;
        let mut discovered = Vec::new();
        let mut path = None;
        let mut head = String::new();
        for field in listing.split(|byte| *byte == 0) {
            if let Some(value) = field.strip_prefix(b"worktree ") {
                path = Some(PathBuf::from(std::str::from_utf8(value).map_err(
                    |error| GitError::new(GitErrorKind::InvalidOutput, error.to_string()),
                )?));
            } else if let Some(value) = field.strip_prefix(b"HEAD ") {
                head = String::from_utf8_lossy(value).into_owned();
            } else if field.is_empty()
                && let Some(path) = path.take()
                && let Ok(path) = path.canonicalize()
                && path
                    != canonical
                        .canonicalize()
                        .unwrap_or_else(|_| canonical.clone())
            {
                if git_common_directory(&path).ok().as_ref() != Some(&common) {
                    continue;
                }
                let mut record = records
                    .iter()
                    .find(|record| {
                        record.state != WorktreeState::Discarded
                            && record.path.canonicalize().ok().as_ref() == Some(&path)
                    })
                    .cloned()
                    .unwrap_or_else(|| TaskWorktree {
                        task_id: editor_core::TaskId::new(format!(
                            "external-{:016x}",
                            stable_path_hash(&path)
                        )),
                        repository_root: canonical.clone(),
                        path: path.clone(),
                        base_commit: head.clone(),
                        state: WorktreeState::Active,
                        conflict: None,
                    });
                // 一覧の正本はGit。保存済みrecordから他Repositoryのpathを認可しない。
                record.repository_root = canonical.clone();
                record.path = path;
                discovered.push(record);
            }
        }
        discovered.sort_by(|left, right| left.task_id.as_str().cmp(right.task_id.as_str()));
        Ok(discovered)
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
        if !self.status(&worktree.path)?.files.is_empty() {
            return Err(GitError::new(
                GitErrorKind::Conflict,
                "未保存・未Commitの変更があるWorktreeは削除できません",
            ));
        }
        if git_text(
            &worktree.path,
            &["branch", "--contains", "HEAD", "--format=%(refname)"],
        )?
        .trim()
        .is_empty()
        {
            return Err(GitError::new(
                GitErrorKind::Conflict,
                "WorktreeのCommitを先にBranchへ保存してください",
            ));
        }
        let path_text = worktree.path.to_string_lossy();
        git_bytes(
            &worktree.repository_root,
            &["worktree", "remove", "--", &path_text],
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

fn git_command(repository: &Path, arguments: &[&str]) -> Command {
    let mut command = Command::new("git");
    command
        .args(["-c", "credential.interactive=never", "--literal-pathspecs"])
        .arg("-C")
        .arg(repository)
        .args(arguments)
        .stdin(Stdio::null())
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .env_remove("GIT_PREFIX")
        .env("GIT_OPTIONAL_LOCKS", "0");
    command
}

fn git_bytes_allow_exit_code(
    repository: &Path,
    arguments: &[&str],
    allowed_failure_code: i32,
) -> Result<Vec<u8>, GitError> {
    let mut command = git_command(repository, arguments);
    let output = git_action::git_output(&mut command, Duration::from_secs(120))
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

fn git_common_directory(repository: &Path) -> Result<PathBuf, GitError> {
    let path = git_text(
        repository,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    PathBuf::from(path.trim())
        .canonicalize()
        .map_err(|error| GitError::new(GitErrorKind::Io, error.to_string()))
}

fn git_repository_gate(repository: &Path) -> Result<Arc<Mutex<()>>, GitError> {
    type Gates = std::collections::HashMap<PathBuf, std::sync::Weak<Mutex<()>>>;
    static GATES: std::sync::OnceLock<Mutex<Gates>> = std::sync::OnceLock::new();
    let directory = git_common_directory(repository)?;
    let mut gates = GATES
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| GitError::new(GitErrorKind::Io, "Repositoryの操作lockが利用できません"))?;
    gates.retain(|_, gate| gate.strong_count() != 0);
    if let Some(gate) = gates.get(&directory).and_then(std::sync::Weak::upgrade) {
        return Ok(gate);
    }
    let gate = Arc::new(Mutex::new(()));
    gates.insert(directory, Arc::downgrade(&gate));
    Ok(gate)
}

fn stable_path_hash(path: &Path) -> u64 {
    path.to_string_lossy()
        .bytes()
        .fold(0xcbf29ce484222325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        })
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
    fn watch_repository(
        &self,
        repository: &Path,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Box<dyn git::GitWatch>, GitError> {
        if !self.gate.is_connected() {
            return Err(GitError::new(
                GitErrorKind::Io,
                "loopback workspace is disconnected",
            ));
        }
        self.inner.watch_repository(repository, notify)
    }
    fn discover_repositories(&self, workspace: &Path) -> Result<Vec<PathBuf>, GitError> {
        if !self.gate.is_connected() {
            return Err(GitError::new(
                GitErrorKind::Io,
                "loopback workspace is disconnected",
            ));
        }
        self.inner.discover_repositories(workspace)
    }
    fn execute(
        &self,
        repository: &Path,
        request: git::GitActionRequest,
    ) -> Result<git::GitActionResult, GitError> {
        if !self.gate.is_connected() {
            return Err(GitError::new(
                GitErrorKind::Io,
                "loopback workspace is disconnected",
            ));
        }
        self.inner.execute(repository, request)
    }
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
        task_id: &editor_core::TaskId,
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

    fn start_with_shell(
        &self,
        cwd: &Path,
        columns: u16,
        rows: u16,
        shell: terminal::TerminalShell,
    ) -> Result<TerminalId, TerminalError> {
        if !self.gate.is_connected() {
            return Err(TerminalError::new("loopback workspace is disconnected"));
        }
        self.inner.start_with_shell(cwd, columns, rows, shell)
    }

    fn input(&self, id: &TerminalId, input: &TerminalInput) -> Result<(), TerminalError> {
        if !self.gate.is_connected() {
            return Err(TerminalError::new("loopback workspace is disconnected"));
        }
        self.inner.input(id, input)
    }

    fn resize(
        &self,
        id: &TerminalId,
        columns: u16,
        rows: u16,
    ) -> Result<TerminalDelta, TerminalError> {
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
        revision: document::Revision,
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
        revision: document::Revision,
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
        revision: document::Revision,
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
        revision: document::Revision,
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
    fn git_command_clears_repository_location_environment() {
        let command = git_command(Path::new("."), &["status"]);
        let removed = command
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(key, _)| key.to_string_lossy().into_owned())
            .collect::<std::collections::HashSet<_>>();
        for name in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_COMMON_DIR",
            "GIT_INDEX_FILE",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "GIT_PREFIX",
        ] {
            assert!(removed.contains(name), "{name} must be removed");
        }
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
        let task = editor_core::TaskId::new("task-git-1");
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

        let task2 = editor_core::TaskId::new("task-git-2");
        let worktree2 = backend.create_worktree(&repository, &task2).unwrap();
        fs::write(worktree2.path.join("note.txt"), "other\n").unwrap();
        let conflict = backend
            .import_file(&worktree2, Path::new("note.txt"))
            .unwrap_err();
        assert_eq!(conflict.kind(), GitErrorKind::Conflict);

        for (index, tree) in [&worktree, &worktree2].into_iter().enumerate() {
            assert!(backend.discard_worktree(tree).is_err());
            assert!(tree.path.join("note.txt").exists());
            for args in [
                vec![
                    "switch".to_owned(),
                    "-c".to_owned(),
                    format!("preserve-{index}"),
                ],
                vec!["add".to_owned(), "note.txt".to_owned()],
                vec![
                    "commit".to_owned(),
                    "-m".to_owned(),
                    "preserve changes".to_owned(),
                ],
            ] {
                assert!(
                    Command::new("git")
                        .arg("-C")
                        .arg(&tree.path)
                        .args(args)
                        .status()
                        .unwrap()
                        .success()
                );
            }
        }
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
