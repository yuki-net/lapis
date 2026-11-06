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
    ClientKind, DocumentEditRequest, DocumentId as ApiDocumentId, DocumentInspectRequest,
    DocumentOpenRequest, DocumentSaveRequest, DocumentTextEdit, DocumentTransaction, EventBody,
    EventEnvelope, FileTreeKind, FileTreeRequest, RequestBody, ResponseBody,
    Revision as ApiRevision, SnapshotReason, SnapshotRequest, TerminalId as ApiTerminalId,
    TerminalInputRequest, TerminalResizeRequest, TerminalSize, TerminalStartRequest,
    TerminalStatus as WireTerminalStatus, TerminalTerminateRequest, WorkbenchGetRequest,
    WorkbenchPutRequest, WorkbenchPutResponse, WorkspaceConnectRequest,
    WorkspaceId as ApiWorkspaceId, WorkspaceRelativePath,
};
use lapis_client_core::{
    AsyncHostConnection,
    browser::{BrowserConnection, WebClientConnectOptions, connect_web_client},
};
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
use lapis_terminal::{
    TerminalBackend, TerminalDelta, TerminalError, TerminalEvent, TerminalId, TerminalInput,
    TerminalRowUpdate, TerminalScrollbackDelta, TerminalSnapshot, TerminalStatus,
};
use lapis_workbench::{
    ConversationViewState, VersionedViewState, WorkbenchReplica, WorkbenchUpdate,
    WorkbenchWritePlan,
};
use lapis_workspace::{
    FileEntry, FileEntryKind, SearchError, SearchHit, WorkspaceError, WorkspaceRepository,
    WorkspaceSearchBackend,
};

pub const ROOT: &str = r"C:\Lapis Host";
const UNAVAILABLE: &str = "Webではこの機能に接続していません";
const MAX_TREE_ENTRIES: usize = 5000;
const MAX_PRELOAD_BYTES: usize = 2 * 1024 * 1024;
const WEB_CAPABILITIES: &[&str] = &[
    "workspace.list",
    "workspace.connect",
    "workspace.sync",
    "workspace.workbench.write",
    "workspace.documents.read",
    "workspace.documents.write",
    "workspace.files.read",
    "workspace.files.write",
    "workspace.terminal.start",
    "workspace.terminal.control",
];

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
    terminals: HashMap<String, TerminalState>,
    workbench: WorkbenchReplica,
    next: u64,
    next_terminal: u64,
    initial: Option<PathBuf>,
}

impl Replica {
    fn new(workspace: WorkspaceId, entries: Vec<FileEntry>) -> Self {
        Self {
            workspace,
            entries,
            documents: HashMap::new(),
            by_path: HashMap::new(),
            terminals: HashMap::new(),
            workbench: WorkbenchReplica::default(),
            next: 1,
            next_terminal: 1,
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

fn versioned_workbench(
    snapshot: lapis_client_api::WorkbenchSnapshot,
) -> Option<VersionedViewState> {
    let state = VersionedViewState {
        revision: snapshot.revision,
        view: serde_json::from_value(snapshot.view).ok(),
    };
    state.is_consistent().then_some(state)
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

struct TerminalState {
    host: Option<ApiTerminalId>,
    snapshot: Option<TerminalSnapshot>,
    events: Vec<TerminalEvent>,
    finished: bool,
}

enum Command {
    Open {
        local: DocumentId,
    },
    Sync {
        local: DocumentId,
    },
    Save {
        local: DocumentId,
    },
    Reload {
        local: DocumentId,
    },
    Discard {
        local: DocumentId,
    },
    Close {
        local: DocumentId,
    },
    StartTerminal {
        local: TerminalId,
        cwd: Option<PathBuf>,
        columns: u16,
        rows: u16,
        shell: lapis_terminal::TerminalShell,
    },
    TerminalInput {
        local: TerminalId,
        input: TerminalInput,
    },
    TerminalResize {
        local: TerminalId,
        columns: u16,
        rows: u16,
    },
    TerminalTerminate {
        local: TerminalId,
    },
    SaveWorkbench {
        view: ConversationViewState,
    },
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
    let (connection, fresh_credential) = connect_web_client(WebClientConnectOptions {
        url: &config.url,
        workspace: api_workspace.clone(),
        client_id: &config.client_id,
        client_name: "lapis-web",
        client_kind: ClientKind::Desktop,
        token: config.token.as_deref(),
        credential: config.credential.as_deref(),
        capabilities: WEB_CAPABILITIES,
    })
    .await?;
    let mut connection = Rc::new(connection);
    let events = Rc::get_mut(&mut connection).and_then(|connection| connection.take_events());
    request(
        &connection,
        RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
            workspace_id: api_workspace.clone(),
        }),
    )
    .await?;
    let entries = fetch_tree(&connection, &api_workspace).await?;
    let terminals = match request(
        &connection,
        RequestBody::SnapshotResync(SnapshotRequest {
            workspace_id: api_workspace.clone(),
            reason: SnapshotReason::Reconnect,
        }),
    )
    .await
    {
        Ok(ResponseBody::SnapshotResync(response)) => response.snapshot.terminals,
        _ => Vec::new(),
    };
    let replica = Rc::new(RefCell::new(Replica::new(workspace.clone(), entries)));
    {
        let mut replica = replica.borrow_mut();
        for terminal in terminals {
            let local = TerminalId::new(format!("web-terminal-{}", replica.next_terminal));
            replica.next_terminal += 1;
            let snapshot = model_terminal_snapshot(&local, &terminal);
            replica.terminals.insert(
                local.as_str().to_owned(),
                TerminalState {
                    host: Some(terminal.terminal_id.clone()),
                    snapshot: Some(snapshot.clone()),
                    events: vec![TerminalEvent::Snapshot(snapshot)],
                    finished: false,
                },
            );
        }
    }
    if let Ok(ResponseBody::WorkbenchGet(snapshot)) = request(
        &connection,
        RequestBody::WorkbenchGet(WorkbenchGetRequest {
            workspace_id: api_workspace.clone(),
        }),
    )
    .await
        && let Some(state) = versioned_workbench(snapshot)
    {
        let _ = replica.borrow_mut().workbench.open(state);
    }
    preload_workbench(&connection, &replica, &api_workspace).await;
    let (commands, receiver) = mpsc::unbounded();
    BACKEND.with(|backend| {
        *backend.borrow_mut() = Some(BackendState {
            workspace,
            replica: replica.clone(),
            commands,
        })
    });
    wasm_bindgen_futures::spawn_local(pump(connection.clone(), receiver, replica.clone()));
    if let Some(events) = events {
        wasm_bindgen_futures::spawn_local(event_pump(connection.clone(), events, replica.clone()));
    }
    preload(&connection, &replica).await?;
    Ok(fresh_credential)
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

async fn preload_workbench(
    connection: &BrowserConnection,
    replica: &Rc<RefCell<Replica>>,
    workspace: &ApiWorkspaceId,
) {
    let paths: Vec<(PathBuf, WorkspaceRelativePath)> = {
        let replica = replica.borrow();
        replica
            .workbench
            .confirmed()
            .and_then(|versioned| versioned.view.as_ref())
            .and_then(|view| view.workspace.as_ref())
            .map(|shared| {
                shared
                    .open_documents
                    .iter()
                    .filter_map(|document| document.path.as_ref())
                    .filter_map(|path| {
                        WorkspaceRelativePath::from_path(Path::new(path.as_str()))
                            .ok()
                            .map(|relative| (Path::new(ROOT).join(path.as_str()), relative))
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    for (path, relative) in paths {
        if replica.borrow().by_path.contains_key(&path) {
            continue;
        }
        let Ok(ResponseBody::DocumentOpen(response)) = request(
            connection,
            RequestBody::DocumentOpen(DocumentOpenRequest {
                workspace_id: workspace.clone(),
                path: relative,
            }),
        )
        .await
        else {
            continue;
        };
        let snapshot = response.document;
        let mut replica = replica.borrow_mut();
        let local = local_id(&mut replica);
        let document = document_from_snapshot(&snapshot, Some(path.clone()));
        replica.by_path.insert(path.clone(), local.clone());
        replica.documents.insert(
            local,
            DocumentState {
                host: Some(snapshot.document_id.clone()),
                path: Some(path),
                document,
                acked: snapshot.content.clone(),
                host_revision: snapshot.revision,
                loaded: true,
                external_change: ExternalChange::Unchanged,
            },
        );
    }
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

async fn event_pump(
    connection: Rc<BrowserConnection>,
    mut events: mpsc::Receiver<EventEnvelope>,
    replica: Rc<RefCell<Replica>>,
) {
    while let Some(event) = events.next().await {
        match event.body {
            EventBody::TerminalDelta { terminal_id, delta } => {
                if let Some(state) = replica
                    .borrow_mut()
                    .terminals
                    .values_mut()
                    .find(|state| state.host.as_ref() == Some(&terminal_id))
                {
                    state.events.push(TerminalEvent::Delta(delta));
                }
            }
            EventBody::TerminalStatus {
                terminal_id,
                status,
            } => {
                if let Some(state) = replica
                    .borrow_mut()
                    .terminals
                    .values_mut()
                    .find(|state| state.host.as_ref() == Some(&terminal_id))
                {
                    match status {
                        WireTerminalStatus::Exited | WireTerminalStatus::Terminated => {
                            state.events.push(TerminalEvent::Exited { code: None });
                            state.finished = true;
                        }
                        WireTerminalStatus::Failed => {
                            state
                                .events
                                .push(TerminalEvent::Failed("Terminalが失敗しました".into()));
                            state.finished = true;
                        }
                        _ => {}
                    }
                }
            }
            EventBody::WorkbenchChanged { .. } => {
                let workspace = api_workspace(&replica.borrow().workspace);
                if let Ok(ResponseBody::WorkbenchGet(snapshot)) = request(
                    &connection,
                    RequestBody::WorkbenchGet(WorkbenchGetRequest {
                        workspace_id: workspace,
                    }),
                )
                .await
                    && let Ok(mut replica) = replica.try_borrow_mut()
                    && let Some(state) = versioned_workbench(snapshot)
                {
                    let _ = replica.workbench.refresh(state);
                }
            }
            _ => {}
        }
    }
}

fn model_terminal_status(status: WireTerminalStatus) -> TerminalStatus {
    match status {
        WireTerminalStatus::Starting | WireTerminalStatus::Running => TerminalStatus::Running,
        WireTerminalStatus::Exited | WireTerminalStatus::Terminated => TerminalStatus::Exited,
        WireTerminalStatus::Failed => TerminalStatus::Failed,
        WireTerminalStatus::Disconnected => TerminalStatus::Disconnected,
    }
}

fn model_terminal_snapshot(
    local: &TerminalId,
    snapshot: &lapis_client_api::TerminalSnapshot,
) -> TerminalSnapshot {
    TerminalSnapshot {
        id: local.clone(),
        cwd: snapshot
            .cwd
            .as_ref()
            .map(|path| Path::new(ROOT).join(path.as_str()))
            .unwrap_or_default(),
        status: model_terminal_status(snapshot.status),
        revision: snapshot.revision,
        screen: snapshot.screen.clone(),
    }
}

fn full_terminal_delta(snapshot: &TerminalSnapshot) -> TerminalDelta {
    TerminalDelta {
        revision: snapshot.revision,
        size: snapshot.screen.size,
        changed_rows: snapshot
            .screen
            .rows
            .iter()
            .enumerate()
            .map(|(row, value)| TerminalRowUpdate {
                row: row as u16,
                value: value.clone(),
            })
            .collect(),
        cursor: snapshot.screen.cursor,
        modes: snapshot.screen.modes,
        scrollback: Some(TerminalScrollbackDelta {
            reset: true,
            dropped: 0,
            rows: snapshot.screen.scrollback.clone(),
        }),
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
        Command::StartTerminal {
            local,
            cwd,
            columns,
            rows,
            shell,
        } => {
            let workspace_id = replica.borrow().workspace.clone();
            let cwd = match cwd.as_deref() {
                Some(path) => match path.strip_prefix(ROOT) {
                    Ok(relative) if relative.as_os_str().is_empty() => None,
                    Ok(relative) => Some(
                        WorkspaceRelativePath::from_path(relative)
                            .map_err(|_| "Terminalのcwdが不正です")?,
                    ),
                    Err(_) => None,
                },
                None => None,
            };
            let body = request(
                connection,
                RequestBody::TerminalStart(TerminalStartRequest {
                    workspace_id: api_workspace(&workspace_id),
                    cwd,
                    command: None,
                    shell,
                    size: TerminalSize { columns, rows },
                }),
            )
            .await?;
            let ResponseBody::TerminalStart(response) = body else {
                return Err("TerminalStart応答が不正です".into());
            };
            let mut replica = replica.borrow_mut();
            if let Some(state) = replica.terminals.get_mut(local.as_str()) {
                state.host = Some(response.terminal.terminal_id.clone());
                let snapshot = model_terminal_snapshot(&local, &response.terminal);
                state.snapshot = Some(snapshot.clone());
                state.events.push(TerminalEvent::Snapshot(snapshot));
            }
        }
        Command::TerminalInput { local, input } => {
            let host = {
                let replica = replica.borrow();
                replica
                    .terminals
                    .get(local.as_str())
                    .and_then(|state| state.host.clone())
            };
            if let Some(host) = host {
                let _ = request(
                    connection,
                    RequestBody::TerminalInput(TerminalInputRequest {
                        terminal_id: host,
                        input,
                    }),
                )
                .await;
            }
        }
        Command::TerminalResize {
            local,
            columns,
            rows,
        } => {
            let host = {
                let replica = replica.borrow();
                replica
                    .terminals
                    .get(local.as_str())
                    .and_then(|state| state.host.clone())
            };
            if let Some(host) = host {
                let body = request(
                    connection,
                    RequestBody::TerminalResize(TerminalResizeRequest {
                        terminal_id: host,
                        size: TerminalSize { columns, rows },
                    }),
                )
                .await?;
                let ResponseBody::TerminalResize(response) = body else {
                    return Err("TerminalResize応答が不正です".into());
                };
                let mut replica = replica.borrow_mut();
                if let Some(state) = replica.terminals.get_mut(local.as_str()) {
                    let snapshot = model_terminal_snapshot(&local, &response.terminal);
                    let delta = full_terminal_delta(&snapshot);
                    state.snapshot = Some(snapshot);
                    state.events.push(TerminalEvent::Delta(delta));
                }
            }
        }
        Command::TerminalTerminate { local } => {
            let host = {
                let replica = replica.borrow();
                replica
                    .terminals
                    .get(local.as_str())
                    .and_then(|state| state.host.clone())
            };
            if let Some(host) = host {
                let _ = request(
                    connection,
                    RequestBody::TerminalTerminate(TerminalTerminateRequest { terminal_id: host }),
                )
                .await;
            }
            if let Some(state) = replica.borrow_mut().terminals.get_mut(local.as_str()) {
                state.events.push(TerminalEvent::Exited { code: None });
                state.finished = true;
            }
        }
        Command::SaveWorkbench { view } => {
            let (workspace, plan) = {
                let mut replica = replica.borrow_mut();
                let workspace = api_workspace(&replica.workspace);
                let plan = replica.workbench.stage(view)?;
                (workspace, plan)
            };
            let WorkbenchWritePlan::Update { revision, view } = plan else {
                return Err("Workbenchが競合しています".into());
            };
            let encoded = lapis_client_api::encode_workbench_view(&view)
                .map_err(|error| error.to_string())?;
            let body = request(
                connection,
                RequestBody::WorkbenchPut(WorkbenchPutRequest {
                    workspace_id: workspace,
                    expected_revision: revision,
                    view: encoded,
                }),
            )
            .await?;
            let update = match body {
                ResponseBody::WorkbenchPut(WorkbenchPutResponse::Updated(snapshot)) => {
                    WorkbenchUpdate::Updated(
                        versioned_workbench(snapshot).ok_or("WorkbenchPut応答が不正です")?,
                    )
                }
                ResponseBody::WorkbenchPut(WorkbenchPutResponse::Conflict(snapshot)) => {
                    WorkbenchUpdate::Conflict(
                        versioned_workbench(snapshot).ok_or("WorkbenchPut応答が不正です")?,
                    )
                }
                _ => return Err("WorkbenchPut応答が不正です".into()),
            };
            replica.borrow_mut().workbench.finish(update)?;
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
            .map_err(DocumentError::io)
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
                loaded: true,
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

fn workspace_from_shared(
    shared: &lapis_workbench::SharedWorkspaceView,
    expected: &WorkspaceId,
    root: PathBuf,
) -> Result<crate::WorkspaceSnapshot, WorkspaceError> {
    if &shared.workspace_id != expected {
        return Err(WorkspaceError::new("Workspace IDが一致しません"));
    }
    let documents = shared
        .open_documents
        .iter()
        .map(|view| crate::DocumentViewState {
            document_id: view.document_id.clone(),
            path: view
                .path
                .as_ref()
                .map(|path| root.join(path.as_str()))
                .unwrap_or_default(),
            cursor_char: usize::try_from(view.cursor_char).unwrap_or(usize::MAX),
            selection_start: usize::try_from(view.selection_start).unwrap_or(usize::MAX),
            selection_end: usize::try_from(view.selection_end).unwrap_or(usize::MAX),
            scroll_x: view.scroll_x,
            scroll_y: view.scroll_y,
            draft_content: view.draft_content.clone(),
        })
        .collect();
    Ok(crate::WorkspaceSnapshot {
        root: Some(root),
        open_documents: documents,
        active_path: shared
            .active_path
            .as_ref()
            .map(|path| Path::new(ROOT).join(path.as_str())),
        active_document_id: shared.active_document_id.clone(),
    })
}

fn shared_workspace(
    snapshot: &crate::WorkspaceSnapshot,
    workspace_id: WorkspaceId,
) -> Result<lapis_workbench::SharedWorkspaceView, WorkspaceError> {
    // wasmにはOSのpath意味論が無いため、絶対path検証を通さず同じ変換だけを行う。
    let root = snapshot
        .root
        .as_ref()
        .ok_or_else(|| WorkspaceError::new("Workspaceがありません"))?;
    let relative = |path: &Path| -> Result<WorkspaceRelativePath, WorkspaceError> {
        let path = path
            .strip_prefix(root)
            .map_err(|_| WorkspaceError::new("Workspace外のpathは共有できません"))?;
        WorkspaceRelativePath::from_path(path)
            .map_err(|error| WorkspaceError::new(error.to_string()))
    };
    let documents = snapshot
        .open_documents
        .iter()
        .map(|view| {
            Ok(lapis_workbench::SharedDocumentView {
                document_id: view.document_id.clone(),
                path: (!view.path.as_os_str().is_empty())
                    .then(|| relative(&view.path))
                    .transpose()?,
                cursor_char: u64::try_from(view.cursor_char)
                    .map_err(|_| WorkspaceError::new("カーソル位置が範囲外です"))?,
                selection_start: u64::try_from(view.selection_start)
                    .map_err(|_| WorkspaceError::new("選択位置が範囲外です"))?,
                selection_end: u64::try_from(view.selection_end)
                    .map_err(|_| WorkspaceError::new("選択位置が範囲外です"))?,
                scroll_x: view.scroll_x,
                scroll_y: view.scroll_y,
                draft_content: view.draft_content.clone(),
            })
        })
        .collect::<Result<Vec<_>, WorkspaceError>>()?;
    Ok(lapis_workbench::SharedWorkspaceView {
        workspace_id,
        open_documents: documents,
        active_path: snapshot.active_path.as_deref().map(relative).transpose()?,
        active_document_id: snapshot.active_document_id.clone(),
    })
}

/// 接続時に取得したHostのWorkbenchから、UIへ復元するレイアウト。
pub fn restore_view() -> Option<lapis_workbench::ConversationViewState> {
    BACKEND.with(|backend| {
        let backend = backend.borrow();
        let state = backend.as_ref()?;
        let replica = state.replica.borrow();
        let view = replica
            .workbench
            .confirmed()
            .and_then(|versioned| versioned.view.as_ref())?
            .clone();
        Some(lapis_workbench::ConversationViewState {
            workspace: None,
            ..view
        })
    })
}

/// 接続時に取得したHostのWorkbenchから、UIへ復元する文書状態を作る。
pub fn restore_snapshot(root: PathBuf) -> Option<crate::WorkspaceSnapshot> {
    BACKEND.with(|backend| {
        let backend = backend.borrow();
        let state = backend.as_ref()?;
        let replica = state.replica.borrow();
        let shared = replica
            .workbench
            .confirmed()
            .and_then(|versioned| versioned.view.as_ref())
            .and_then(|view| view.workspace.as_ref())?;
        workspace_from_shared(shared, &replica.workspace, root).ok()
    })
}

impl crate::ConversationRepository for WebBackend {
    fn load(&self) -> Result<crate::ConversationState, WorkspaceError> {
        self.with(|state| {
            let replica = state.replica.borrow();
            let Some(view) = replica
                .workbench
                .confirmed()
                .and_then(|versioned| versioned.view.as_ref())
            else {
                return Ok((Vec::new(), Vec::new(), None));
            };
            let Some(workspace) = view.workspace.as_ref().and_then(|shared| {
                workspace_from_shared(shared, &replica.workspace, PathBuf::from(ROOT)).ok()
            }) else {
                return Ok((Vec::new(), Vec::new(), None));
            };
            let id = lapis_editor_core::ConversationId::new("web-conversation");
            let record = ConversationRecord {
                id: id.clone(),
                title: "Web".to_owned(),
                workspace,
                view: ConversationViewState {
                    workspace: None,
                    ..view.clone()
                },
                selected_execution: None,
                terminals: Vec::new(),
                environment: None,
                source_role: crate::SourceRole::Primary,
                other_workspace: None,
                other_terminals: Vec::new(),
                other_execution: None,
                parked_sources: Vec::new(),
            };
            Ok((Vec::new(), vec![record], Some(id)))
        })
        .map_err(WorkspaceError::new)?
    }

    fn save(
        &self,
        _: &crate::ConversationState,
        _: &[Project],
        records: &[ConversationRecord],
        active: &lapis_editor_core::ConversationId,
    ) -> Result<(), WorkspaceError> {
        self.with(|state| {
            let record = records
                .iter()
                .find(|record| &record.id == active)
                .or_else(|| records.first())
                .ok_or_else(|| WorkspaceError::new("保存するConversationがありません"))?;
            let replica = state.replica.borrow();
            if replica.workbench.conflict().is_some() {
                return Err(WorkspaceError::new("Workbenchが競合しています"));
            }
            let workspace = shared_workspace(&record.workspace, replica.workspace.clone())?;
            let view = ConversationViewState {
                layout: record.view.layout.clone(),
                show_panel_controls: record.view.show_panel_controls,
                workbench: record.view.workbench.clone(),
                workspace: Some(workspace),
            };
            let _ = state
                .commands
                .unbounded_send(Command::SaveWorkbench { view });
            Ok(())
        })
        .map_err(WorkspaceError::new)?
    }
}

impl WorkspaceSearchBackend for WebBackend {
    fn search(
        &self,
        _: &Path,
        _: &str,
        _: &std::sync::atomic::AtomicBool,
    ) -> Result<Vec<SearchHit>, SearchError> {
        Err(SearchError::new(UNAVAILABLE))
    }
}

impl TaskConnection for WebBackend {
    fn load(&self, _: &WorkspaceId) -> Result<Vec<TaskSnapshot>, TaskError> {
        Err(TaskError::new(UNAVAILABLE))
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
        Err(GitError::new(GitErrorKind::Io, UNAVAILABLE))
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
    fn start(&self, cwd: &Path, columns: u16, rows: u16) -> Result<TerminalId, TerminalError> {
        self.start_with_shell(cwd, columns, rows, lapis_terminal::TerminalShell::Default)
    }

    fn start_with_shell(
        &self,
        cwd: &Path,
        columns: u16,
        rows: u16,
        shell: lapis_terminal::TerminalShell,
    ) -> Result<TerminalId, TerminalError> {
        self.with(|state| {
            let mut replica = state.replica.borrow_mut();
            let local = TerminalId::new(format!("web-terminal-{}", replica.next_terminal));
            replica.next_terminal += 1;
            replica.terminals.insert(
                local.as_str().to_owned(),
                TerminalState {
                    host: None,
                    snapshot: None,
                    events: Vec::new(),
                    finished: false,
                },
            );
            let _ = state.commands.unbounded_send(Command::StartTerminal {
                local: local.clone(),
                cwd: Some(cwd.to_owned()),
                columns,
                rows,
                shell,
            });
            local
        })
        .map_err(TerminalError::new)
    }

    fn input(&self, id: &TerminalId, input: &TerminalInput) -> Result<(), TerminalError> {
        self.with(|state| {
            let _ = state.commands.unbounded_send(Command::TerminalInput {
                local: id.clone(),
                input: input.clone(),
            });
        })
        .map_err(TerminalError::new)
    }

    fn resize(
        &self,
        id: &TerminalId,
        columns: u16,
        rows: u16,
    ) -> Result<TerminalDelta, TerminalError> {
        let delta = self
            .with(|state| {
                let replica = state.replica.borrow();
                let delta = match replica
                    .terminals
                    .get(id.as_str())
                    .and_then(|state| state.snapshot.as_ref())
                {
                    Some(snapshot) => full_terminal_delta(snapshot),
                    None => return Err("Terminalがまだ開始していません".to_owned()),
                };
                let _ = state.commands.unbounded_send(Command::TerminalResize {
                    local: id.clone(),
                    columns,
                    rows,
                });
                Ok(delta)
            })
            .map_err(TerminalError::new)?;
        delta.map_err(TerminalError::new)
    }

    fn poll(&self, id: &TerminalId) -> Result<Vec<TerminalEvent>, TerminalError> {
        let events = self
            .with(|state| {
                let mut replica = state.replica.borrow_mut();
                let terminal = replica
                    .terminals
                    .get_mut(id.as_str())
                    .ok_or_else(|| "Terminalがありません".to_owned())?;
                if terminal.finished && terminal.events.is_empty() {
                    return Err("Terminalは終了しました".to_owned());
                }
                Ok(std::mem::take(&mut terminal.events))
            })
            .map_err(TerminalError::new)?;
        events.map_err(TerminalError::new)
    }

    fn list(&self, _: &Path) -> Result<Vec<TerminalSnapshot>, TerminalError> {
        self.with(|state| {
            state
                .replica
                .borrow()
                .terminals
                .values()
                .filter_map(|state| state.snapshot.clone())
                .collect()
        })
        .map_err(TerminalError::new)
    }

    fn attach(&self, _: &Path, id: &TerminalId) -> Result<TerminalSnapshot, TerminalError> {
        let snapshot = self
            .with(|state| {
                state
                    .replica
                    .borrow()
                    .terminals
                    .get(id.as_str())
                    .and_then(|state| state.snapshot.clone())
                    .ok_or_else(|| "Terminalがありません".to_owned())
            })
            .map_err(TerminalError::new)?;
        snapshot.map_err(TerminalError::new)
    }

    fn terminate(&self, id: &TerminalId) -> Result<(), TerminalError> {
        self.with(|state| {
            let _ = state
                .commands
                .unbounded_send(Command::TerminalTerminate { local: id.clone() });
        })
        .map_err(TerminalError::new)
    }
}
