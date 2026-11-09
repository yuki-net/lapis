//! WebSocket 接続の要求・イベント処理と、ローカル複製への反映。

use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
};

use client_api::{
    DocumentEditRequest, DocumentInspectRequest, DocumentOpenRequest, DocumentSaveRequest,
    DocumentTextEdit, DocumentTransaction, EventBody, EventEnvelope, FileTreeKind, FileTreeRequest,
    RequestBody, ResponseBody, Revision as ApiRevision, SnapshotReason, SnapshotRequest,
    TerminalInputRequest, TerminalResizeRequest, TerminalSize, TerminalStartRequest,
    TerminalStatus as WireTerminalStatus, TerminalTerminateRequest, WorkbenchGetRequest,
    WorkbenchPutRequest, WorkbenchPutResponse, WorkspaceId as ApiWorkspaceId,
    WorkspaceRelativePath,
};
use client_core::{AsyncHostConnection, browser::BrowserConnection};
use document::{ExternalChange, FileFingerprint};
use editor_core::DocumentId;
use futures_channel::mpsc;
use futures_util::StreamExt;
use terminal::{TerminalEvent, TerminalId, TerminalInput};
use workbench::{ConversationViewState, WorkbenchUpdate, WorkbenchWritePlan};
use workspace::{FileEntry, FileEntryKind};

use super::{
    MAX_PRELOAD_BYTES, MAX_TREE_ENTRIES, ROOT, api_workspace,
    state::{
        DocumentState, Replica, TerminalState, diff_chars, document_from_snapshot, local_id,
        model_terminal_snapshot, versioned_workbench,
    },
};

pub(super) enum Command {
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
        shell: terminal::TerminalShell,
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
    RetryWorkbenchLocal,
}

pub(super) async fn request(
    connection: &BrowserConnection,
    body: RequestBody,
) -> Result<ResponseBody, String> {
    match AsyncHostConnection::request(connection, body).await? {
        ResponseBody::Error(error) => Err(error.code.as_str().to_owned()),
        body => Ok(body),
    }
}

pub(super) async fn fetch_tree(
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

pub(super) async fn preload_workbench(
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

pub(super) async fn preload(
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
    let workspace = replica.borrow().workspace.clone();
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

pub(super) async fn pump(
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

pub(super) async fn event_pump(
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
            EventBody::SnapshotRequired { .. } => {
                // イベントを取りこぼした。全 Terminal を Host の Snapshot で置き換える。
                sync_terminals(&connection, &replica).await;
            }
            _ => {}
        }
    }
}

/// `snapshot.required` の後で、Terminal の cache と表示用イベントを最新へ置き換える。
async fn sync_terminals(connection: &Rc<BrowserConnection>, replica: &Rc<RefCell<Replica>>) {
    let workspace = api_workspace(&replica.borrow().workspace);
    let Ok(ResponseBody::SnapshotResync(response)) = request(
        connection,
        RequestBody::SnapshotResync(SnapshotRequest {
            workspace_id: workspace,
            reason: SnapshotReason::EventGap,
        }),
    )
    .await
    else {
        return;
    };
    let Ok(mut replica) = replica.try_borrow_mut() else {
        return;
    };
    for terminal in response.snapshot.terminals {
        if let Some(state) = replica
            .terminals
            .values_mut()
            .find(|state| state.host.as_ref() == Some(&terminal.terminal_id))
        {
            let local = state
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.id.clone())
                .unwrap_or_else(|| TerminalId::new(terminal.terminal_id.as_str()));
            let snapshot = model_terminal_snapshot(&local, &terminal);
            state.snapshot = Some(snapshot.clone());
            state.events.clear();
            state.events.push(TerminalEvent::Snapshot(snapshot));
            state.finished = false;
            continue;
        }
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
                    RequestBody::DocumentDiscard(client_api::DocumentDiscardRequest {
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
                    RequestBody::DocumentClose(client_api::DocumentCloseRequest {
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
                // 画面は terminal.delta イベントで届くため、応答は受理確認だけ。
                request(
                    connection,
                    RequestBody::TerminalResize(TerminalResizeRequest {
                        terminal_id: host,
                        size: TerminalSize { columns, rows },
                    }),
                )
                .await?;
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
            let plan = replica.borrow_mut().workbench.stage(view)?;
            write_workbench(connection, replica, plan).await?;
        }
        Command::RetryWorkbenchLocal => {
            let pending = replica.borrow_mut().workbench.prepare_retry()?;
            let plan = replica.borrow_mut().workbench.stage(pending)?;
            write_workbench(connection, replica, plan).await?;
        }
    }
    Ok(())
}

/// 確定済みrevisionを基準にWorkbenchを書き込み、応答で競合状態を更新する。
async fn write_workbench(
    connection: &BrowserConnection,
    replica: &Rc<RefCell<Replica>>,
    plan: WorkbenchWritePlan,
) -> Result<(), String> {
    let (workspace, revision, view) = {
        let replica = replica.borrow();
        let workspace = api_workspace(&replica.workspace);
        match plan {
            WorkbenchWritePlan::Update { revision, view } => (workspace, revision, view),
            WorkbenchWritePlan::Conflict(_) => return Err("Workbenchが競合しています".into()),
        }
    };
    let encoded = client_api::encode_workbench_view(&view).map_err(|error| error.to_string())?;
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
