//! Hostへ接続し、GPUI UIの同期Clientへローカル複製を提供する。
//!
//! 読み取りは複製から即座に返し、書き込みは複製へ反映してHostへ非同期送信する。
//! Hostが拒否した場合は正本を取得し直し、次のsnapshotでUIへ反映する。

use std::{
    cell::RefCell,
    collections::HashMap,
    ops::Range,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

use crate::{
    ConversationRecord, Project, WorkspaceDialog, WorkspaceSnapshot, WorkspaceStateRepository,
};
use futures_channel::mpsc;
use futures_util::StreamExt;
use lapis_client_api::{
    AuthenticateRequest, CURRENT_PROTOCOL, CapabilityId, CapabilitySet, ClientHello, ClientId,
    ClientKind, ClientMessage, CredentialId, DocumentEditRequest, DocumentId as ApiDocumentId,
    DocumentInspectRequest, DocumentOpenRequest, DocumentSaveRequest, DocumentTextEdit,
    DocumentTransaction, FileTreeKind, FileTreeRequest, PairRequest, ProtocolRange, RequestBody,
    ResponseBody, Revision as ApiRevision, ServerMessage, WorkspaceConnectRequest,
    WorkspaceId as ApiWorkspaceId, WorkspaceRelativePath,
};
use lapis_client_core::{AsyncHostConnection, browser::BrowserConnection};
use lapis_document::{
    Document, DocumentCache, DocumentConnection, DocumentError, Encoding, ExternalChange,
    FileFingerprint, Revision,
};
use lapis_editor_core::{DocumentId, WorkspaceId};
use lapis_git::{
    GitConnection, GitDiffSnapshot, GitError, GitErrorKind, GitSnapshot, WorktreeSnapshot,
};
use lapis_lsp::{
    CompletionItem, DefinitionTarget, Diagnostic, LanguageServerBackend, LspError, LspPosition,
};
use lapis_settings::{GlobalSettings, GlobalSettingsRepository, SettingsError};
use lapis_task::{TaskConnection, TaskControl, TaskError, TaskSnapshot, TaskStart};
use lapis_terminal::{TerminalBackend, TerminalDelta, TerminalError, TerminalEvent, TerminalId};
use lapis_workspace::{
    FileEntry, FileEntryKind, SearchError, SearchHit, WorkspaceError, WorkspaceRepository,
    WorkspaceSearchBackend,
};

pub const ROOT: &str = "/Lapis Host";
const UNAVAILABLE: &str = "Webではこの機能に接続していません";
const MAX_TREE_ENTRIES: usize = 5000;
const MAX_PRELOAD_BYTES: usize = 2 * 1024 * 1024;

thread_local! {
    static BACKEND: RefCell<Option<BackendState>> = const { RefCell::new(None) };
}

struct BackendState {
    workspace: WorkspaceId,
    replica: Rc<RefCell<Replica>>,
    commands: mpsc::UnboundedSender<Command>,
}

struct Replica {
    workspace: WorkspaceId,
    entries: Vec<FileEntry>,
    documents: HashMap<DocumentId, DocumentState>,
    by_path: HashMap<PathBuf, DocumentId>,
    next: u64,
    initial: Option<PathBuf>,
}

impl Replica {
    fn new(workspace: WorkspaceId, entries: Vec<FileEntry>) -> Self {
        Self {
            workspace,
            entries,
            documents: HashMap::new(),
            by_path: HashMap::new(),
            next: 1,
            initial: None,
        }
    }
}

fn cache_of(
    local: &DocumentId,
    document: &Document,
    external_change: ExternalChange,
) -> DocumentCache {
    DocumentCache {
        reused: true,
        id: local.clone(),
        document: document.clone(),
        can_undo: document.can_undo(),
        can_redo: document.can_redo(),
        external_change,
    }
}

struct DocumentState {
    host: Option<ApiDocumentId>,
    path: Option<PathBuf>,
    document: Document,
    acked: String,
    host_revision: ApiRevision,
    loaded: bool,
    external_change: ExternalChange,
}

enum Command {
    Open { local: DocumentId },
    Sync { local: DocumentId },
    Save { local: DocumentId },
    Reload { local: DocumentId },
    Discard { local: DocumentId },
    Close { local: DocumentId },
}

/// Host接続の設定。ページのクエリかlocalStorageから渡される。
pub struct HostConfig {
    pub url: String,
    pub workspace: String,
    pub client_id: String,
    pub token: Option<String>,
    pub credential: Option<String>,
}

/// Hostへ接続し、初期状態を取得してバックエンドを設置する。pairing時はcredentialを返す。
pub async fn connect(config: HostConfig) -> Result<Option<String>, String> {
    let api_workspace =
        ApiWorkspaceId::try_new(config.workspace.clone()).map_err(|_| "Workspace IDが不正です")?;
    let workspace = WorkspaceId::new(api_workspace.as_str());
    let hello = hello(&config.client_id)?;
    let mut fresh_credential = None;
    let auth = match config.credential.as_deref() {
        Some(credential) => {
            let stored: StoredCredential =
                serde_json::from_str(credential).map_err(|_| "credentialの形式が不正です")?;
            let id = CredentialId::parse(stored.id).map_err(|_| "credential IDが不正です")?;
            let secret = decode_hex(&stored.secret)?;
            let request = AuthenticateRequest::new(hello, api_workspace.clone(), &id, &secret);
            ClientMessage::Authenticate(request)
        }
        None => {
            let token = config.token.as_deref().ok_or("pairing tokenが必要です")?;
            let secret = decode_hex(token)?;
            let request = PairRequest::new(hello, &secret);
            ClientMessage::Pair(request)
        }
    };
    let (connection, response) = BrowserConnection::connect(&config.url, auth)
        .await
        .map_err(|error| error.to_string())?;
    if let ServerMessage::Paired(mut paired) = response {
        let id = paired
            .credential_id()
            .map_err(|_| "credential IDが不正です")?;
        let secret = paired
            .take_secret()
            .map_err(|_| "credential secretが不正です")?;
        fresh_credential = Some(
            serde_json::to_string(&StoredCredential {
                id: id.as_str().to_owned(),
                secret: encode_hex(&secret),
            })
            .map_err(|_| "credentialを保存できません")?,
        );
    }
    let connection = Rc::new(connection);
    request(
        &connection,
        RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
            workspace_id: api_workspace.clone(),
        }),
    )
    .await?;
    let entries = fetch_tree(&connection, &api_workspace).await?;
    let replica = Rc::new(RefCell::new(Replica::new(workspace.clone(), entries)));
    let (commands, receiver) = mpsc::unbounded();
    BACKEND.with(|backend| {
        *backend.borrow_mut() = Some(BackendState {
            workspace,
            replica: replica.clone(),
            commands,
        })
    });
    wasm_bindgen_futures::spawn_local(pump(connection.clone(), receiver, replica.clone()));
    preload(&connection, &replica).await?;
    Ok(fresh_credential)
}

fn hello(client_id: &str) -> Result<ClientHello, String> {
    let capabilities = [
        "workspace.list",
        "workspace.connect",
        "workspace.sync",
        "workspace.workbench.write",
        "workspace.documents.read",
        "workspace.documents.write",
        "workspace.files.read",
        "workspace.files.write",
    ]
    .into_iter()
    .map(|capability| {
        CapabilityId::try_new(capability).map_err(|_| "capabilityが不正です".to_owned())
    })
    .collect::<Result<Vec<_>, _>>()?;
    Ok(ClientHello {
        protocol: ProtocolRange::exact(CURRENT_PROTOCOL),
        client_id: ClientId::try_new(client_id).map_err(|_| "Client IDが不正です")?,
        client_name: "lapis-web".into(),
        client_kind: ClientKind::Desktop,
        requested_capabilities: CapabilitySet::try_new(capabilities)
            .map_err(|_| "capabilityが不正です")?,
    })
}

async fn request(
    connection: &BrowserConnection,
    body: RequestBody,
) -> Result<ResponseBody, String> {
    match AsyncHostConnection::request(connection, body).await? {
        ResponseBody::Error(error) => Err(error.code.as_str().to_owned()),
        body => Ok(body),
    }
}

async fn fetch_tree(
    connection: &BrowserConnection,
    workspace: &ApiWorkspaceId,
) -> Result<Vec<FileEntry>, String> {
    let mut entries = Vec::new();
    let mut pending = vec![None::<WorkspaceRelativePath>];
    while let Some(directory) = pending.pop() {
        let body = request(
            connection,
            RequestBody::FileTree(FileTreeRequest {
                workspace_id: workspace.clone(),
                path: directory,
            }),
        )
        .await?;
        let ResponseBody::FileTree(response) = body else {
            return Err("FileTree応答が不正です".into());
        };
        for entry in response.entries {
            let relative = PathBuf::from(entry.path.as_str());
            let root = Path::new(ROOT);
            entries.push(FileEntry {
                path: root.join(&relative),
                depth: relative.components().count().saturating_sub(1),
                relative_path: relative,
                kind: match entry.kind {
                    FileTreeKind::File => FileEntryKind::File,
                    FileTreeKind::Directory => FileEntryKind::Directory,
                    FileTreeKind::Symlink => continue,
                },
            });
            if entry.kind == FileTreeKind::Directory {
                pending.push(Some(entry.path));
            }
            if entries.len() >= MAX_TREE_ENTRIES {
                return Ok(entries);
            }
        }
    }
    Ok(entries)
}

async fn preload(
    connection: &BrowserConnection,
    replica: &Rc<RefCell<Replica>>,
) -> Result<(), String> {
    let initial = {
        let replica = replica.borrow();
        replica
            .entries
            .iter()
            .find(|entry| {
                entry.kind == FileEntryKind::File && entry.relative_path == Path::new("README.md")
            })
            .or_else(|| {
                replica
                    .entries
                    .iter()
                    .find(|entry| entry.kind == FileEntryKind::File)
            })
            .map(|entry| (entry.path.clone(), entry.relative_path.clone()))
    };
    let Some((path, relative)) = initial else {
        return Ok(());
    };
    let workspace = BACKEND.with(|backend| {
        backend
            .borrow()
            .as_ref()
            .map(|state| state.workspace.clone())
    });
    let Some(workspace) = workspace else {
        return Ok(());
    };
    let body = request(
        connection,
        RequestBody::DocumentOpen(DocumentOpenRequest {
            workspace_id: api_workspace(&workspace),
            path: WorkspaceRelativePath::from_path(&relative).map_err(|_| "pathが不正です")?,
        }),
    )
    .await?;
    let ResponseBody::DocumentOpen(response) = body else {
        return Err("DocumentOpen応答が不正です".into());
    };
    let snapshot = response.document;
    if snapshot.content.len() > MAX_PRELOAD_BYTES {
        return Ok(());
    }
    let mut replica = replica.borrow_mut();
    let local = local_id(&mut replica);
    let document = document_from_snapshot(&snapshot, Some(path.clone()));
    let state = DocumentState {
        host: Some(snapshot.document_id.clone()),
        path: Some(path.clone()),
        document,
        acked: snapshot.content.clone(),
        host_revision: snapshot.revision,
        loaded: true,
        external_change: ExternalChange::Unchanged,
    };
    replica.by_path.insert(path.clone(), local.clone());
    replica.documents.insert(local, state);
    replica.initial = Some(path);
    Ok(())
}

fn local_id(replica: &mut Replica) -> DocumentId {
    let id = DocumentId::new(format!("web-{}", replica.next));
    replica.next += 1;
    id
}

fn api_workspace(id: &WorkspaceId) -> ApiWorkspaceId {
    ApiWorkspaceId::try_new(id.as_str()).expect("workspace id is valid")
}

fn document_from_snapshot(
    snapshot: &lapis_client_api::DocumentSnapshot,
    path: Option<PathBuf>,
) -> Document {
    Document::from_snapshot(
        path,
        snapshot.content.clone(),
        match snapshot.encoding {
            lapis_client_api::DocumentEncoding::Utf8 => Encoding::Utf8,
            lapis_client_api::DocumentEncoding::Utf8Bom => Encoding::Utf8Bom,
            lapis_client_api::DocumentEncoding::Utf16Le => Encoding::Utf16Le,
            lapis_client_api::DocumentEncoding::Utf16Be => Encoding::Utf16Be,
        },
        Revision::new(snapshot.revision.value()),
        snapshot.dirty,
    )
}

async fn pump(
    connection: Rc<BrowserConnection>,
    mut commands: mpsc::UnboundedReceiver<Command>,
    replica: Rc<RefCell<Replica>>,
) {
    while let Some(command) = commands.next().await {
        if let Err(error) = handle(&connection, &replica, command).await {
            web_sys::console::warn_1(&format!("Host同期: {error}").into());
        }
    }
}

async fn handle(
    connection: &BrowserConnection,
    replica: &Rc<RefCell<Replica>>,
    command: Command,
) -> Result<(), String> {
    match command {
        Command::Open { local } => {
            let (relative, workspace) = {
                let replica = replica.borrow();
                let state = replica
                    .documents
                    .get(&local)
                    .ok_or("Documentがありません")?;
                let path = state.path.as_ref().ok_or("pathがありません")?;
                let relative = path
                    .strip_prefix(ROOT)
                    .map_err(|_| "Workspace外のpathです")?;
                (
                    WorkspaceRelativePath::from_path(relative).map_err(|_| "pathが不正です")?,
                    api_workspace(&replica.workspace),
                )
            };
            let body = request(
                connection,
                RequestBody::DocumentOpen(DocumentOpenRequest {
                    workspace_id: workspace,
                    path: relative,
                }),
            )
            .await?;
            let ResponseBody::DocumentOpen(response) = body else {
                return Err("DocumentOpen応答が不正です".into());
            };
            let snapshot = response.document;
            let mut replica = replica.borrow_mut();
            if let Some(state) = replica.documents.get_mut(&local) {
                state.host = Some(snapshot.document_id.clone());
                state.document = document_from_snapshot(&snapshot, state.path.clone());
                state.acked = snapshot.content.clone();
                state.host_revision = snapshot.revision;
                state.loaded = true;
                state.external_change = ExternalChange::Unchanged;
            }
        }
        Command::Sync { local } => loop {
            let (host, base, transaction, sent) = {
                let replica = replica.borrow();
                let Some(state) = replica.documents.get(&local) else {
                    return Ok(());
                };
                let (Some(host), true) = (state.host.clone(), state.loaded) else {
                    return Ok(());
                };
                let text = state.document.content_for_save();
                if text == state.acked {
                    return Ok(());
                }
                let (start, end, replacement) = diff_chars(&state.acked, &text);
                let edit = DocumentTextEdit::try_new(start, end, replacement.to_owned())
                    .map_err(|_| "編集範囲が不正です")?;
                let transaction =
                    DocumentTransaction::try_new(vec![edit]).map_err(|_| "編集範囲が不正です")?;
                (host, state.host_revision, transaction, text)
            };
            match request(
                connection,
                RequestBody::DocumentEdit(DocumentEditRequest {
                    document_id: host,
                    base_revision: base,
                    transaction,
                }),
            )
            .await
            {
                Ok(ResponseBody::DocumentEdit(response)) => {
                    let mut replica = replica.borrow_mut();
                    if let Some(state) = replica.documents.get_mut(&local) {
                        state.host_revision = response.revision;
                        state.acked = sent;
                    }
                    let pending = replica
                        .documents
                        .get(&local)
                        .is_some_and(|state| state.document.content_for_save() != state.acked);
                    if !pending {
                        return Ok(());
                    }
                }
                Ok(_) => return Err("DocumentEdit応答が不正です".into()),
                Err(_) => {
                    refresh_document(connection, replica, &local).await?;
                    return Ok(());
                }
            }
        },
        Command::Save { local } => {
            let (host, base, path) = {
                let replica = replica.borrow();
                let Some(state) = replica.documents.get(&local) else {
                    return Ok(());
                };
                let Some(host) = state.host.clone() else {
                    return Ok(());
                };
                (host, state.host_revision, state.path.clone())
            };
            match request(
                connection,
                RequestBody::DocumentSave(DocumentSaveRequest {
                    document_id: host,
                    base_revision: base,
                }),
            )
            .await
            {
                Ok(ResponseBody::DocumentSave(response)) => {
                    let mut replica = replica.borrow_mut();
                    if let Some(state) = replica.documents.get_mut(&local) {
                        state.host_revision = response.revision;
                        if let Some(path) = path {
                            let bytes = state.document.content_for_save().into_bytes();
                            let fingerprint = FileFingerprint::new(bytes.len() as u64, None, 0);
                            state.document.mark_saved(path, fingerprint);
                        }
                    }
                }
                Ok(_) => return Err("DocumentSave応答が不正です".into()),
                Err(_) => refresh_document(connection, replica, &local).await?,
            }
        }
        Command::Reload { local } => {
            refresh_document(connection, replica, &local).await?;
        }
        Command::Discard { local } => {
            let (host, base) = {
                let replica = replica.borrow();
                match replica.documents.get(&local) {
                    Some(state) => (state.host.clone(), state.host_revision),
                    None => (None, ApiRevision::new(0)),
                }
            };
            if let Some(host) = host {
                let _ = request(
                    connection,
                    RequestBody::DocumentDiscard(lapis_client_api::DocumentDiscardRequest {
                        document_id: host,
                        base_revision: base,
                    }),
                )
                .await;
                refresh_document(connection, replica, &local).await?;
            }
        }
        Command::Close { local } => {
            let host = {
                let replica = replica.borrow();
                replica
                    .documents
                    .get(&local)
                    .and_then(|state| state.host.clone())
            };
            if let Some(host) = host {
                let _ = request(
                    connection,
                    RequestBody::DocumentClose(lapis_client_api::DocumentCloseRequest {
                        document_id: host,
                    }),
                )
                .await;
            }
            let mut replica = replica.borrow_mut();
            if let Some(state) = replica.documents.remove(&local)
                && let Some(path) = state.path
            {
                replica.by_path.remove(&path);
            }
        }
    }
    Ok(())
}

async fn refresh_document(
    connection: &BrowserConnection,
    replica: &Rc<RefCell<Replica>>,
    local: &DocumentId,
) -> Result<(), String> {
    let host = {
        let replica = replica.borrow();
        replica
            .documents
            .get(local)
            .and_then(|state| state.host.clone())
    };
    let Some(host) = host else {
        return Ok(());
    };
    let body = request(
        connection,
        RequestBody::DocumentInspect(DocumentInspectRequest { document_id: host }),
    )
    .await?;
    let ResponseBody::DocumentInspect(response) = body else {
        return Err("DocumentInspect応答が不正です".into());
    };
    let snapshot = response.document;
    let mut replica = replica.borrow_mut();
    if let Some(state) = replica.documents.get_mut(local) {
        state.document = document_from_snapshot(&snapshot, state.path.clone());
        state.acked = snapshot.content.clone();
        state.host_revision = snapshot.revision;
        state.external_change = ExternalChange::Modified;
    }
    Ok(())
}

fn diff_chars(previous: &str, next: &str) -> (u64, u64, String) {
    let previous_chars: Vec<char> = previous.chars().collect();
    let next_chars: Vec<char> = next.chars().collect();
    let mut start = 0;
    while start < previous_chars.len()
        && start < next_chars.len()
        && previous_chars[start] == next_chars[start]
    {
        start += 1;
    }
    let mut tail = 0;
    while tail < previous_chars.len().saturating_sub(start)
        && tail < next_chars.len().saturating_sub(start)
        && previous_chars[previous_chars.len() - 1 - tail]
            == next_chars[next_chars.len() - 1 - tail]
    {
        tail += 1;
    }
    let end = previous_chars.len() - tail;
    let replacement: String = next_chars[start..next_chars.len() - tail].iter().collect();
    (start as u64, end as u64, replacement)
}

#[derive(serde::Deserialize, serde::Serialize)]
struct StoredCredential {
    id: String,
    secret: String,
}

fn decode_hex(value: &str) -> Result<[u8; 32], String> {
    let value = value.trim();
    if value.len() != 64 {
        return Err("secretの長さが不正です".into());
    }
    let mut output = [0u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let digit = |byte: u8| match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            _ => None,
        };
        let high = digit(pair[0]).ok_or("secretが不正です")?;
        let low = digit(pair[1]).ok_or("secretが不正です")?;
        output[index] = (high << 4) | low;
    }
    Ok(output)
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

/// 起動時に開くHost上のファイル。
pub fn initial_path() -> Option<PathBuf> {
    BACKEND.with(|backend| {
        backend
            .borrow()
            .as_ref()
            .and_then(|state| state.replica.borrow().initial.clone())
    })
}

/// UIへ渡す同期Client。状態はthread_localにあるため、この型自体は空。
#[derive(Clone, Copy)]
pub struct WebBackend;

impl WebBackend {
    fn with<T>(self, operation: impl FnOnce(&mut BackendState) -> T) -> Result<T, String> {
        BACKEND.with(|backend| {
            let mut backend = backend.borrow_mut();
            let state = backend.as_mut().ok_or("Hostに接続していません")?;
            Ok(operation(state))
        })
    }
}

impl DocumentConnection for WebBackend {
    fn fork(&self) -> Arc<dyn DocumentConnection> {
        Arc::new(*self)
    }

    fn workspace(&self, _: &Path) -> Result<WorkspaceId, DocumentError> {
        self.with(|state| state.workspace.clone())
            .map_err(|error| DocumentError::io(error))
    }

    fn create(&self, _: Option<&Path>) -> Result<DocumentCache, DocumentError> {
        self.with(|state| {
            let mut replica = state.replica.borrow_mut();
            let local = local_id(&mut replica);
            let document = Document::new();
            let state = DocumentState {
                host: None,
                path: None,
                document,
                acked: String::new(),
                host_revision: ApiRevision::new(0),
                loaded: false,
                external_change: ExternalChange::Unchanged,
            };
            let cache = cache_of(&local, &state.document, state.external_change);
            replica.documents.insert(local, state);
            cache
        })
        .map_err(DocumentError::io)
    }

    fn open(&self, path: &Path) -> Result<DocumentCache, DocumentError> {
        self.with(|state| {
            let mut replica = state.replica.borrow_mut();
            if let Some(local) = replica.by_path.get(path).cloned() {
                let state = replica
                    .documents
                    .get(&local)
                    .ok_or_else(|| DocumentError::io("Documentがありません"))?;
                return Ok(cache_of(&local, &state.document, state.external_change));
            }
            let local = local_id(&mut replica);
            let document = Document::from_snapshot(
                Some(path.to_owned()),
                String::new(),
                Encoding::Utf8,
                Revision::new(0),
                false,
            );
            let document_state = DocumentState {
                host: None,
                path: Some(path.to_owned()),
                document,
                acked: String::new(),
                host_revision: ApiRevision::new(0),
                loaded: false,
                external_change: ExternalChange::Unchanged,
            };
            let cache = cache_of(
                &local,
                &document_state.document,
                document_state.external_change,
            );
            replica.by_path.insert(path.to_owned(), local.clone());
            replica.documents.insert(local.clone(), document_state);
            let _ = state.commands.unbounded_send(Command::Open { local });
            Ok(cache)
        })
        .map_err(DocumentError::io)?
    }

    fn attach(&self, id: &DocumentId) -> Result<DocumentCache, DocumentError> {
        self.with(|state| {
            let replica = state.replica.borrow();
            let document = replica
                .documents
                .get(id)
                .ok_or_else(|| DocumentError::io("Documentがありません"))?;
            Ok(cache_of(id, &document.document, document.external_change))
        })
        .map_err(DocumentError::io)?
    }

    fn edit(
        &self,
        id: &DocumentId,
        revision: Revision,
        range: Range<usize>,
        replacement: &str,
    ) -> Result<DocumentCache, DocumentError> {
        self.with(|state| {
            let mut replica = state.replica.borrow_mut();
            let document = replica
                .documents
                .get_mut(id)
                .ok_or_else(|| DocumentError::io("Documentがありません"))?;
            if !document.loaded {
                return Err(DocumentError::io("Hostから読み込み中です"));
            }
            if document.document.revision() != revision {
                return Err(DocumentError::conflict(
                    "Documentのrevisionが競合しています",
                ));
            }
            document
                .document
                .replace_char_range(range, replacement)
                .map_err(|error| DocumentError::io(error.to_string()))?;
            let cache = cache_of(id, &document.document, document.external_change);
            let _ = state
                .commands
                .unbounded_send(Command::Sync { local: id.clone() });
            Ok(cache)
        })
        .map_err(DocumentError::io)?
    }

    fn history(
        &self,
        id: &DocumentId,
        revision: Revision,
        undo: bool,
    ) -> Result<DocumentCache, DocumentError> {
        self.with(|state| {
            let mut replica = state.replica.borrow_mut();
            let document = replica
                .documents
                .get_mut(id)
                .ok_or_else(|| DocumentError::io("Documentがありません"))?;
            if document.document.revision() != revision {
                return Err(DocumentError::conflict(
                    "Documentのrevisionが競合しています",
                ));
            }
            if undo {
                document.document.undo();
            } else {
                document.document.redo();
            }
            let cache = cache_of(id, &document.document, document.external_change);
            let _ = state
                .commands
                .unbounded_send(Command::Sync { local: id.clone() });
            Ok(cache)
        })
        .map_err(DocumentError::io)?
    }

    fn save(
        &self,
        id: &DocumentId,
        revision: Revision,
        _path: Option<PathBuf>,
    ) -> Result<DocumentCache, DocumentError> {
        self.with(|state| {
            let mut replica = state.replica.borrow_mut();
            let document = replica
                .documents
                .get_mut(id)
                .ok_or_else(|| DocumentError::io("Documentがありません"))?;
            if document.document.revision() != revision {
                return Err(DocumentError::conflict(
                    "Documentのrevisionが競合しています",
                ));
            }
            let cache = cache_of(id, &document.document, document.external_change);
            let _ = state
                .commands
                .unbounded_send(Command::Save { local: id.clone() });
            Ok(cache)
        })
        .map_err(DocumentError::io)?
    }

    fn reload(&self, id: &DocumentId, revision: Revision) -> Result<DocumentCache, DocumentError> {
        self.with(|state| {
            let mut replica = state.replica.borrow_mut();
            let document = replica
                .documents
                .get_mut(id)
                .ok_or_else(|| DocumentError::io("Documentがありません"))?;
            if document.document.revision() != revision {
                return Err(DocumentError::conflict(
                    "Documentのrevisionが競合しています",
                ));
            }
            let cache = cache_of(id, &document.document, document.external_change);
            let _ = state
                .commands
                .unbounded_send(Command::Reload { local: id.clone() });
            Ok(cache)
        })
        .map_err(DocumentError::io)?
    }

    fn discard(&self, id: &DocumentId, revision: Revision) -> Result<DocumentCache, DocumentError> {
        self.with(|state| {
            let mut replica = state.replica.borrow_mut();
            let document = replica
                .documents
                .get_mut(id)
                .ok_or_else(|| DocumentError::io("Documentがありません"))?;
            if document.document.revision() != revision {
                return Err(DocumentError::conflict(
                    "Documentのrevisionが競合しています",
                ));
            }
            let cache = cache_of(id, &document.document, document.external_change);
            let _ = state
                .commands
                .unbounded_send(Command::Discard { local: id.clone() });
            Ok(cache)
        })
        .map_err(DocumentError::io)?
    }

    fn close(&self, id: &DocumentId) -> Result<(), DocumentError> {
        self.with(|state| {
            let mut replica = state.replica.borrow_mut();
            if let Some(document) = replica.documents.get(id)
                && let Some(path) = document.path.clone()
            {
                replica.by_path.remove(&path);
            }
            replica.documents.remove(id);
            let _ = state
                .commands
                .unbounded_send(Command::Close { local: id.clone() });
            Ok(())
        })
        .map_err(DocumentError::io)?
    }
}

impl WorkspaceRepository for WebBackend {
    fn create_directory(&self, _: &Path, _: &str) -> Result<(), WorkspaceError> {
        Err(WorkspaceError::new("Webからはフォルダーを作成できません"))
    }

    fn list_tree(&self, _: &Path) -> Result<Vec<FileEntry>, WorkspaceError> {
        self.with(|state| state.replica.borrow().entries.clone())
            .map_err(WorkspaceError::new)
    }
}

impl WorkspaceDialog for WebBackend {
    fn choose_workspace_path(&self) -> Option<PathBuf> {
        Some(ROOT.into())
    }
    fn choose_file_path(&self) -> Option<PathBuf> {
        None
    }
    fn choose_save_path(&self, suggested_name: &str) -> Option<PathBuf> {
        Some(Path::new(ROOT).join(suggested_name))
    }
}

impl WorkspaceStateRepository for WebBackend {
    fn load(&self) -> Result<Option<WorkspaceSnapshot>, WorkspaceError> {
        Ok(None)
    }
    fn save(&self, _: &WorkspaceSnapshot) -> Result<(), WorkspaceError> {
        Ok(())
    }
}

impl GlobalSettingsRepository for WebBackend {
    fn load(&self) -> Result<GlobalSettings, SettingsError> {
        Ok(GlobalSettings::default())
    }
    fn save(&self, _: &GlobalSettings) -> Result<(), SettingsError> {
        Ok(())
    }
}

impl crate::ConversationRepository for WebBackend {
    fn load(&self) -> Result<crate::ConversationState, WorkspaceError> {
        Ok((
            Vec::<Project>::new(),
            Vec::<ConversationRecord>::new(),
            None,
        ))
    }
    fn save(
        &self,
        _: &crate::ConversationState,
        _: &[Project],
        _: &[ConversationRecord],
        _: &lapis_editor_core::ConversationId,
    ) -> Result<(), WorkspaceError> {
        Ok(())
    }
}

impl WorkspaceSearchBackend for WebBackend {
    fn search(
        &self,
        _: &Path,
        _: &str,
        _: &std::sync::atomic::AtomicBool,
    ) -> Result<Vec<SearchHit>, SearchError> {
        Ok(Vec::new())
    }
}

impl TaskConnection for WebBackend {
    fn load(&self, _: &WorkspaceId) -> Result<Vec<TaskSnapshot>, TaskError> {
        Ok(Vec::new())
    }
    fn start(&self, _: &WorkspaceId, _: TaskStart) -> Result<TaskSnapshot, TaskError> {
        Err(TaskError::new(UNAVAILABLE))
    }
    fn control(
        &self,
        _: &WorkspaceId,
        _: &lapis_editor_core::ExecutionId,
        _: &TaskControl,
    ) -> Result<(), TaskError> {
        Err(TaskError::new(UNAVAILABLE))
    }
}

impl GitConnection for WebBackend {
    fn status(
        &self,
        _: &WorkspaceId,
        _: Option<&lapis_editor_core::TaskId>,
    ) -> Result<GitSnapshot, GitError> {
        Err(GitError::new(GitErrorKind::Io, UNAVAILABLE))
    }
    fn diff(
        &self,
        _: &WorkspaceId,
        _: Option<&lapis_editor_core::TaskId>,
        _: &str,
    ) -> Result<GitDiffSnapshot, GitError> {
        Err(GitError::new(GitErrorKind::Io, UNAVAILABLE))
    }
    fn worktrees(&self, _: &WorkspaceId) -> Result<Vec<WorktreeSnapshot>, GitError> {
        Ok(Vec::new())
    }
    fn create_worktree(&self, _: &WorkspaceId) -> Result<WorktreeSnapshot, GitError> {
        Err(GitError::new(GitErrorKind::Io, UNAVAILABLE))
    }
    fn import_file(
        &self,
        _: &WorkspaceId,
        _: &lapis_editor_core::TaskId,
        _: &str,
    ) -> Result<WorktreeSnapshot, GitError> {
        Err(GitError::new(GitErrorKind::Io, UNAVAILABLE))
    }
    fn discard_worktree(
        &self,
        _: &WorkspaceId,
        _: &lapis_editor_core::TaskId,
    ) -> Result<WorktreeSnapshot, GitError> {
        Err(GitError::new(GitErrorKind::Io, UNAVAILABLE))
    }
}

impl LanguageServerBackend for WebBackend {
    fn start(&self, _: &Path) -> Result<(), LspError> {
        Err(LspError::new(UNAVAILABLE))
    }
    fn did_open(&self, _: &Path, _: &str, _: Revision) -> Result<(), LspError> {
        Err(LspError::new(UNAVAILABLE))
    }
    fn did_change(&self, _: &Path, _: &str, _: Revision) -> Result<(), LspError> {
        Err(LspError::new(UNAVAILABLE))
    }
    fn diagnostics(&self) -> Result<Vec<Diagnostic>, LspError> {
        Ok(Vec::new())
    }
    fn completion(
        &self,
        _: &Path,
        _: LspPosition,
        _: Revision,
    ) -> Result<Vec<CompletionItem>, LspError> {
        Err(LspError::new(UNAVAILABLE))
    }
    fn definition(
        &self,
        _: &Path,
        _: LspPosition,
        _: Revision,
    ) -> Result<Option<DefinitionTarget>, LspError> {
        Ok(None)
    }
    fn shutdown(&self) -> Result<(), LspError> {
        Ok(())
    }
}

impl TerminalBackend for WebBackend {
    fn start(&self, _: &Path, _: u16, _: u16) -> Result<TerminalId, TerminalError> {
        Err(TerminalError::new(UNAVAILABLE))
    }
    fn input(
        &self,
        _: &TerminalId,
        _: &lapis_terminal::TerminalInput,
    ) -> Result<(), TerminalError> {
        Err(TerminalError::new(UNAVAILABLE))
    }
    fn resize(&self, _: &TerminalId, _: u16, _: u16) -> Result<TerminalDelta, TerminalError> {
        Err(TerminalError::new(UNAVAILABLE))
    }
    fn poll(&self, _: &TerminalId) -> Result<Vec<TerminalEvent>, TerminalError> {
        Err(TerminalError::new(UNAVAILABLE))
    }
    fn terminate(&self, _: &TerminalId) -> Result<(), TerminalError> {
        Err(TerminalError::new(UNAVAILABLE))
    }
}
