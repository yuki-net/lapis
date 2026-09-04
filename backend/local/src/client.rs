use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use lapis_app_services::WorkspaceRepository;
use lapis_backend_state::{
    BackendEventReceiver, BackendService, BackendServiceError, BackendSession,
};
use lapis_client_api::{
    DocumentCreateRequest, DocumentEncoding, DocumentOpenRequest, DocumentSnapshot,
    DocumentTextEdit, DocumentTransaction, EventBody, FileTreeKind, FileTreeRequest, RequestBody,
    RequestEnvelope, RequestId, ResponseBody, SessionId, TerminalInputRequest,
    TerminalResizeRequest, TerminalSize, TerminalStartRequest, TerminalStatus,
    TerminalTerminateRequest, WorkspaceConnectRequest, WorkspaceId, WorkspaceRelativePath,
};
use lapis_document::{
    DocumentError, DocumentErrorKind, DocumentRepository, FileData, FileFingerprint,
};
use lapis_terminal::{TerminalBackend, TerminalError, TerminalEvent, TerminalId};
use lapis_workspace::{FileEntry, FileEntryKind, WorkspaceError};

#[derive(Debug)]
pub enum BackendClientError {
    Service(BackendServiceError),
    Rejected(String),
    InvalidPath(String),
}

impl std::fmt::Display for BackendClientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Service(error) => write!(formatter, "backend service unavailable: {error}"),
            Self::Rejected(error) => write!(formatter, "backend rejected request: {error}"),
            Self::InvalidPath(error) => write!(formatter, "invalid workspace path: {error}"),
        }
    }
}

impl std::error::Error for BackendClientError {}

#[derive(Clone)]
pub struct BackendClient {
    service: BackendService,
    session: BackendSession,
    root: PathBuf,
    files: Arc<dyn DocumentRepository>,
    events: Arc<Mutex<Option<BackendEventReceiver>>>,
    next_request_id: Arc<std::sync::atomic::AtomicU64>,
}

impl BackendClient {
    pub fn connect(
        service: BackendService,
        session_id: SessionId,
        workspace_id: WorkspaceId,
        root: PathBuf,
        files: Arc<dyn DocumentRepository>,
    ) -> Result<Self, BackendClientError> {
        let root = root
            .canonicalize()
            .map(normalize_path)
            .map_err(|error| BackendClientError::InvalidPath(error.to_string()))?;
        let session = BackendSession::new(session_id, workspace_id.clone());
        let client = Self {
            service,
            session,
            root,
            files,
            // The connection is established before subscribing so a local client has the same
            // event boundary as a remote session.
            events: Arc::new(Mutex::new(None)),
            next_request_id: Arc::new(std::sync::atomic::AtomicU64::new(1)),
        };
        let response = client.request(RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
            workspace_id,
        }))?;
        if !matches!(response, ResponseBody::WorkspaceConnect(_)) {
            return Err(BackendClientError::Rejected(response_error(&response)));
        }
        let events = client
            .service
            .subscribe(client.session.clone())
            .map_err(BackendClientError::Service)?;
        *client
            .events
            .lock()
            .map_err(|_| BackendClientError::Rejected("event state lock failed".to_owned()))? =
            Some(events);
        Ok(client)
    }

    pub fn workspace_repository(&self) -> BackendWorkspaceRepository {
        BackendWorkspaceRepository {
            client: self.clone(),
        }
    }

    pub fn terminal_backend(&self) -> BackendTerminalBackend {
        BackendTerminalBackend {
            client: self.clone(),
        }
    }

    pub fn disconnect(&self) -> Result<(), BackendClientError> {
        self.service
            .disconnect(self.session.session_id.clone())
            .map_err(BackendClientError::Service)
    }

    fn request(&self, body: RequestBody) -> Result<ResponseBody, BackendClientError> {
        let number = self
            .next_request_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let request = RequestEnvelope {
            request_id: RequestId::try_new(format!("desktop-local-{number}"))
                .expect("generated local request id must be valid"),
            body,
        };
        let response = self
            .service
            .dispatch(self.session.clone(), request)
            .map_err(BackendClientError::Service)?;
        match response.body {
            ResponseBody::Error(error) => Err(BackendClientError::Rejected(protocol_error(&error))),
            body => Ok(body),
        }
    }

    fn relative_path(&self, path: &Path) -> Result<WorkspaceRelativePath, BackendClientError> {
        let absolute = if path.is_absolute() {
            path.to_owned()
        } else {
            self.root.join(path)
        };
        let absolute = normalize_path(absolute);
        let relative = absolute
            .strip_prefix(&self.root)
            .map_err(|_| BackendClientError::InvalidPath(path.display().to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        WorkspaceRelativePath::parse(relative)
            .map_err(|error| BackendClientError::InvalidPath(error.to_string()))
    }

    fn document(&self, path: &Path) -> Result<DocumentSnapshot, BackendClientError> {
        let path = self.relative_path(path)?;
        match self.request(RequestBody::DocumentOpen(DocumentOpenRequest {
            workspace_id: self.session.workspace_id.clone(),
            path,
        }))? {
            ResponseBody::DocumentOpen(response) => Ok(response.document),
            body => Err(BackendClientError::Rejected(response_error(&body))),
        }
    }

    fn poll_events(&self) -> Result<Vec<EventBody>, TerminalError> {
        let mut events = self
            .events
            .lock()
            .map_err(|_| TerminalError::new("Backend event state lock failed"))?;
        let Some(events) = events.as_mut() else {
            return Ok(Vec::new());
        };
        let mut bodies = Vec::new();
        loop {
            match events.recv_timeout(Duration::from_millis(0)) {
                Ok(event) => bodies.push(event.body),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        Ok(bodies)
    }
}

#[derive(Clone)]
pub struct BackendWorkspaceRepository {
    client: BackendClient,
}

impl WorkspaceRepository for BackendWorkspaceRepository {
    fn list_tree(&self, root: &Path) -> Result<Vec<FileEntry>, WorkspaceError> {
        let requested_root = root.to_owned();
        let mut entries = Vec::new();
        self.list_directory(None, &requested_root, 0, &mut entries)?;
        Ok(entries)
    }
}

impl BackendWorkspaceRepository {
    fn list_directory(
        &self,
        relative: Option<WorkspaceRelativePath>,
        absolute: &Path,
        depth: usize,
        entries: &mut Vec<FileEntry>,
    ) -> Result<(), WorkspaceError> {
        let response = self
            .client
            .request(RequestBody::FileTree(FileTreeRequest {
                workspace_id: self.client.session.workspace_id.clone(),
                path: relative.clone(),
            }))
            .map_err(|error| WorkspaceError::new(error.to_string()))?;
        let ResponseBody::FileTree(response) = response else {
            return Err(WorkspaceError::new(
                "backend returned an unexpected file tree response",
            ));
        };
        let mut children = response.entries;
        children.sort_by_key(|entry| {
            (
                entry.kind != FileTreeKind::Directory,
                entry.name.to_lowercase(),
            )
        });
        for child in children {
            let child_absolute = absolute.join(&child.name);
            let kind = match child.kind {
                FileTreeKind::Directory => FileEntryKind::Directory,
                FileTreeKind::File | FileTreeKind::Symlink => FileEntryKind::File,
            };
            entries.push(FileEntry {
                path: child_absolute.clone(),
                relative_path: PathBuf::from(child.path.as_str()),
                kind,
                depth,
            });
            if child.kind == FileTreeKind::Directory {
                self.list_directory(
                    Some(child.path),
                    &child_absolute,
                    depth.saturating_add(1),
                    entries,
                )?;
            }
        }
        Ok(())
    }
}

impl DocumentRepository for BackendWorkspaceRepository {
    fn read_file(&self, path: &Path) -> Result<FileData, DocumentError> {
        let document = self.client.document(path).map_err(document_error)?;
        let bytes = encode_content(&document.content, document.encoding)?;
        let fingerprint = self
            .client
            .files
            .fingerprint(path)?
            .ok_or_else(|| DocumentError::io("backend document is missing on disk"))?;
        Ok(FileData { bytes, fingerprint })
    }

    fn write_file(
        &self,
        path: &Path,
        content: &[u8],
        expected: Option<&FileFingerprint>,
    ) -> Result<FileFingerprint, DocumentError> {
        let actual = self.client.files.fingerprint(path)?;
        if expected != actual.as_ref() {
            return Err(DocumentError::conflict(
                "保存前に外部変更を検出しました。再読み込みまたは差分確認が必要です",
            ));
        }
        let existing = if actual.is_some() {
            Some(self.client.document(path).map_err(document_error)?)
        } else {
            self.client.document(path).ok()
        };
        let (document, content) = match existing {
            Some(document) => {
                let content = decode_content(content, document.encoding)?;
                (document, content)
            }
            None => {
                let (content, encoding) = decode_content_with_bom(content)?;
                let relative = self.client.relative_path(path).map_err(document_error)?;
                let response = self
                    .client
                    .request(RequestBody::DocumentCreate(DocumentCreateRequest {
                        workspace_id: self.client.session.workspace_id.clone(),
                        path: relative,
                        encoding,
                        content: content.clone(),
                    }))
                    .map_err(document_error)?;
                let ResponseBody::DocumentCreate(response) = response else {
                    return Err(DocumentError::io(
                        "backend returned an unexpected document response",
                    ));
                };
                (response.document, content)
            }
        };
        let replacement = (content != document.content).then_some(content);
        let revision = if let Some(content) = replacement {
            let end = u64::try_from(document.content.chars().count())
                .map_err(|_| DocumentError::io("document is too large"))?;
            let edit = DocumentTextEdit::try_new(0, end, content)
                .map_err(|_| DocumentError::io("invalid document edit"))?;
            let transaction = DocumentTransaction::try_new(vec![edit])
                .map_err(|_| DocumentError::io("invalid document transaction"))?;
            let response = self
                .client
                .request(RequestBody::DocumentEdit(
                    lapis_client_api::DocumentEditRequest {
                        document_id: document.document_id.clone(),
                        base_revision: document.revision,
                        transaction,
                    },
                ))
                .map_err(document_error)?;
            let ResponseBody::DocumentEdit(response) = response else {
                return Err(DocumentError::io(
                    "backend returned an unexpected edit response",
                ));
            };
            response.revision
        } else {
            document.revision
        };
        self.client
            .request(RequestBody::DocumentSave(
                lapis_client_api::DocumentSaveRequest {
                    document_id: document.document_id,
                    base_revision: revision,
                },
            ))
            .map_err(document_error)?;
        self.client
            .files
            .fingerprint(path)?
            .ok_or_else(|| DocumentError::io("保存後のファイルを確認できません"))
    }

    fn fingerprint(&self, path: &Path) -> Result<Option<FileFingerprint>, DocumentError> {
        self.client.files.fingerprint(path)
    }
}

#[derive(Clone)]
pub struct BackendTerminalBackend {
    client: BackendClient,
}

impl TerminalBackend for BackendTerminalBackend {
    fn start(&self, cwd: &Path, columns: u16, rows: u16) -> Result<TerminalId, TerminalError> {
        let cwd = self
            .client
            .relative_path(cwd)
            .map_err(|error| TerminalError::new(error.to_string()))?;
        let response = self
            .client
            .request(RequestBody::TerminalStart(TerminalStartRequest {
                workspace_id: self.client.session.workspace_id.clone(),
                cwd: Some(cwd),
                command: None,
                size: TerminalSize { columns, rows },
            }))
            .map_err(|error| TerminalError::new(error.to_string()))?;
        let ResponseBody::TerminalStart(response) = response else {
            return Err(TerminalError::new(
                "backend returned an unexpected terminal response",
            ));
        };
        Ok(TerminalId::new(response.terminal.terminal_id.as_str()))
    }

    fn input(&self, id: &TerminalId, bytes: &[u8]) -> Result<(), TerminalError> {
        let response = self
            .client
            .request(RequestBody::TerminalInput(TerminalInputRequest {
                terminal_id: lapis_client_api::TerminalId::try_new(id.as_str())
                    .map_err(|error| TerminalError::new(error.to_string()))?,
                data: String::from_utf8_lossy(bytes).into_owned(),
            }))
            .map_err(|error| TerminalError::new(error.to_string()))?;
        if matches!(response, ResponseBody::TerminalInput(_)) {
            Ok(())
        } else {
            Err(TerminalError::new(
                "backend returned an unexpected terminal response",
            ))
        }
    }

    fn resize(&self, id: &TerminalId, columns: u16, rows: u16) -> Result<(), TerminalError> {
        let response = self
            .client
            .request(RequestBody::TerminalResize(TerminalResizeRequest {
                terminal_id: lapis_client_api::TerminalId::try_new(id.as_str())
                    .map_err(|error| TerminalError::new(error.to_string()))?,
                size: TerminalSize { columns, rows },
            }))
            .map_err(|error| TerminalError::new(error.to_string()))?;
        if matches!(response, ResponseBody::TerminalResize(_)) {
            Ok(())
        } else {
            Err(TerminalError::new(
                "backend returned an unexpected terminal response",
            ))
        }
    }

    fn poll(&self, id: &TerminalId) -> Result<Vec<TerminalEvent>, TerminalError> {
        let mut output = Vec::new();
        for event in self.client.poll_events()? {
            match event {
                EventBody::TerminalOutput {
                    terminal_id, data, ..
                } if terminal_id.as_str() == id.as_str() => {
                    output.push(TerminalEvent::Output(data))
                }
                EventBody::TerminalStatus {
                    terminal_id,
                    status,
                } if terminal_id.as_str() == id.as_str() => match status {
                    TerminalStatus::Exited => output.push(TerminalEvent::Exited { code: None }),
                    TerminalStatus::Failed => {
                        output.push(TerminalEvent::Failed("terminal failed".to_owned()))
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        Ok(output)
    }

    fn terminate(&self, id: &TerminalId) -> Result<(), TerminalError> {
        let response = self
            .client
            .request(RequestBody::TerminalTerminate(TerminalTerminateRequest {
                terminal_id: lapis_client_api::TerminalId::try_new(id.as_str())
                    .map_err(|error| TerminalError::new(error.to_string()))?,
            }))
            .map_err(|error| TerminalError::new(error.to_string()))?;
        if matches!(response, ResponseBody::TerminalTerminate(_)) {
            Ok(())
        } else {
            Err(TerminalError::new(
                "backend returned an unexpected terminal response",
            ))
        }
    }
}

fn response_error(response: &ResponseBody) -> String {
    match response {
        ResponseBody::Error(error) => protocol_error(error),
        other => format!(
            "unexpected response for {}",
            other.method().unwrap_or("unknown")
        ),
    }
}

fn protocol_error(error: &lapis_client_api::ProtocolError) -> String {
    match &error.detail {
        Some(detail) => format!("{}: {detail}", error.code.as_str()),
        None => error.code.as_str().to_owned(),
    }
}

fn normalize_path(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let value = path.to_string_lossy();
        if let Some(value) = value.strip_prefix(r"\\?\") {
            return PathBuf::from(value);
        }
    }
    path
}

fn document_error(error: BackendClientError) -> DocumentError {
    match error {
        BackendClientError::Rejected(message) if message.contains("conflict") => {
            DocumentError::conflict(message)
        }
        BackendClientError::Rejected(message) => DocumentError::io(message),
        BackendClientError::Service(error) => DocumentError::io(error.to_string()),
        BackendClientError::InvalidPath(message) => {
            DocumentError::new(DocumentErrorKind::Io, message)
        }
    }
}

fn encode_content(content: &str, encoding: DocumentEncoding) -> Result<Vec<u8>, DocumentError> {
    Ok(match encoding {
        DocumentEncoding::Utf8 => content.as_bytes().to_vec(),
        DocumentEncoding::Utf8Bom => [b"\xef\xbb\xbf".as_slice(), content.as_bytes()].concat(),
        DocumentEncoding::Utf16Le => encode_utf16(content, true),
        DocumentEncoding::Utf16Be => encode_utf16(content, false),
    })
}

fn decode_content(bytes: &[u8], encoding: DocumentEncoding) -> Result<String, DocumentError> {
    let bytes = match encoding {
        DocumentEncoding::Utf8Bom => bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes),
        DocumentEncoding::Utf16Le => bytes.strip_prefix(&[0xff, 0xfe]).unwrap_or(bytes),
        DocumentEncoding::Utf16Be => bytes.strip_prefix(&[0xfe, 0xff]).unwrap_or(bytes),
        DocumentEncoding::Utf8 => bytes,
    };
    match encoding {
        DocumentEncoding::Utf8 | DocumentEncoding::Utf8Bom => String::from_utf8(bytes.to_vec())
            .map_err(|error| {
                DocumentError::new(DocumentErrorKind::InvalidEncoding, error.to_string())
            }),
        DocumentEncoding::Utf16Le | DocumentEncoding::Utf16Be => {
            if !bytes.len().is_multiple_of(2) {
                return Err(DocumentError::new(
                    DocumentErrorKind::InvalidEncoding,
                    "UTF-16ファイルのbyte数が奇数です",
                ));
            }
            let units = bytes.chunks_exact(2).map(|pair| {
                let pair = [pair[0], pair[1]];
                if encoding == DocumentEncoding::Utf16Le {
                    u16::from_le_bytes(pair)
                } else {
                    u16::from_be_bytes(pair)
                }
            });
            String::from_utf16(&units.collect::<Vec<_>>()).map_err(|error| {
                DocumentError::new(DocumentErrorKind::InvalidEncoding, error.to_string())
            })
        }
    }
}

fn decode_content_with_bom(bytes: &[u8]) -> Result<(String, DocumentEncoding), DocumentError> {
    let encoding = if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        DocumentEncoding::Utf8Bom
    } else if bytes.starts_with(&[0xff, 0xfe]) {
        DocumentEncoding::Utf16Le
    } else if bytes.starts_with(&[0xfe, 0xff]) {
        DocumentEncoding::Utf16Be
    } else {
        DocumentEncoding::Utf8
    };
    Ok((decode_content(bytes, encoding)?, encoding))
}

fn encode_utf16(content: &str, little_endian: bool) -> Vec<u8> {
    content
        .encode_utf16()
        .flat_map(|unit| {
            if little_endian {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            }
        })
        .collect()
}
