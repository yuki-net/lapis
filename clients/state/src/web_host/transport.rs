//! WebSocket 接続の要求・イベント処理と、ローカル複製への反映。

use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::Rc,
};

use client_api::{
    DocumentClaimRequest, DocumentEditRequest, DocumentInspectRequest, DocumentOpenRequest,
    DocumentReleaseRequest, DocumentSaveRequest, DocumentTextEdit, DocumentTransaction, EventBody,
    EventEnvelope, FileTreeKind, FileTreeRequest, RequestBody, ResponseBody,
    Revision as ApiRevision, SnapshotReason, SnapshotRequest, TerminalClaimRequest,
    TerminalInputRequest, TerminalReleaseRequest, TerminalResizeRequest, TerminalSize,
    TerminalStartRequest, TerminalStatus as WireTerminalStatus, TerminalTerminateRequest,
    WorkbenchGetRequest, WorkbenchPutRequest, WorkbenchPutResponse, WorkspaceId as ApiWorkspaceId,
    WorkspaceRelativePath, WorkspaceViewSubscribeRequest, WorkspaceViewUpdateRequest,
};
use client_core::{AsyncHostConnection, browser::BrowserConnection};
use document::{ExternalChange, FileFingerprint};
use editor_core::{DocumentId, SessionId};
use futures_channel::mpsc;
use futures_util::StreamExt;
use terminal::TerminalEvent;
use workbench::{WorkbenchUpdate, WorkbenchWritePlan};
use workspace::{FileEntry, FileEntryKind};

use crate::web_switch::Command;

use super::{MAX_PRELOAD_BYTES, MAX_TREE_ENTRIES, ROOT, api_workspace};
use crate::web_replica::{
    DocumentState, Replica, diff_chars, document_from_snapshot, local_id, model_terminal_snapshot,
    owner_of, versioned_workbench,
};

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
                owner: owner_of(&snapshot),
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
        owner: owner_of(&snapshot),
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
    changes: Rc<RefCell<Vec<mpsc::UnboundedSender<()>>>>,
    document_changes: Rc<RefCell<Vec<mpsc::UnboundedSender<()>>>>,
    session_view_changes: Rc<RefCell<Vec<mpsc::UnboundedSender<()>>>>,
) {
    while let Some(command) = commands.next().await {
        if let Err(error) = handle(
            &connection,
            &replica,
            &changes,
            &document_changes,
            &session_view_changes,
            command,
        )
        .await
        {
            web_sys::console::warn_1(&format!("Host同期: {error}").into());
        }
    }
}

pub(super) fn notify_changes(changes: &RefCell<Vec<mpsc::UnboundedSender<()>>>) {
    changes
        .borrow_mut()
        .retain(|changes| changes.unbounded_send(()).is_ok());
}

pub(super) fn notify_documents(documents: &RefCell<Vec<mpsc::UnboundedSender<()>>>) {
    documents
        .borrow_mut()
        .retain(|documents| documents.unbounded_send(()).is_ok());
}

pub(super) fn notify_session_view(view: &RefCell<Vec<mpsc::UnboundedSender<()>>>) {
    view.borrow_mut()
        .retain(|view| view.unbounded_send(()).is_ok());
}

pub(super) async fn event_pump(
    connection: Rc<BrowserConnection>,
    mut events: mpsc::Receiver<EventEnvelope>,
    replica: Rc<RefCell<Replica>>,
    changes: Rc<RefCell<Vec<mpsc::UnboundedSender<()>>>>,
    document_changes: Rc<RefCell<Vec<mpsc::UnboundedSender<()>>>>,
    session_view_changes: Rc<RefCell<Vec<mpsc::UnboundedSender<()>>>>,
) {
    while let Some(event) = events.next().await {
        // 画面や状態の変化を UI に知らせる。UI は通知で terminal の poll を呼ぶ。
        notify_changes(&changes);
        match event.body {
            EventBody::TerminalDelta { terminal_id, delta } => {
                replica
                    .borrow_mut()
                    .apply_terminal_event(&terminal_id, TerminalEvent::Delta(delta));
            }
            EventBody::TerminalStatus {
                terminal_id,
                status,
            } => {
                let event = match status {
                    WireTerminalStatus::Exited | WireTerminalStatus::Terminated => {
                        Some(TerminalEvent::Exited { code: None })
                    }
                    WireTerminalStatus::Failed => {
                        Some(TerminalEvent::Failed("Terminalが失敗しました".into()))
                    }
                    _ => None,
                };
                if let Some(event) = event {
                    replica
                        .borrow_mut()
                        .apply_terminal_event(&terminal_id, event);
                }
            }
            EventBody::TerminalOwner { terminal_id, owner } => {
                replica.borrow_mut().apply_terminal_event(
                    &terminal_id,
                    TerminalEvent::Owner {
                        owner: owner.map(|owner| owner.as_str().to_owned()),
                    },
                );
            }
            EventBody::DocumentOwner { document_id, owner } => {
                if let Some(state) = replica
                    .borrow_mut()
                    .documents
                    .values_mut()
                    .find(|state| state.host.as_ref() == Some(&document_id))
                {
                    state.owner = owner.map(|owner| SessionId::new(owner.as_str()));
                }
            }
            EventBody::DocumentReplaced { document } => {
                if let Some(state) = replica
                    .borrow_mut()
                    .documents
                    .values_mut()
                    .find(|state| state.host.as_ref() == Some(&document.document_id))
                {
                    state.owner = owner_of(&document);
                }
            }
            // 保存された内容へ追従するため、開いている文書だけ正本を取り直す。
            // 未保存の入力を抱えた文書は、解決を急がず外部変更として知らせる。
            EventBody::DocumentSaved { document_id, .. } => {
                let local = replica
                    .borrow()
                    .documents
                    .iter()
                    .find(|(_, state)| state.host.as_ref() == Some(&document_id))
                    .map(|(local, _)| local.clone());
                if let Some(local) = local {
                    let preserve_local = replica
                        .borrow()
                        .documents
                        .get(&local)
                        .is_some_and(|state| state.document.content_for_save() != state.acked);
                    if let Err(error) =
                        refresh_document(&connection, &replica, &local, preserve_local).await
                    {
                        web_sys::console::warn_1(
                            &format!("保存後の文書を再取得できません: {error}").into(),
                        );
                    }
                    notify_documents(&document_changes);
                }
            }
            EventBody::WorkbenchChanged { .. } => {
                let workspace = api_workspace(&replica.borrow().workspace);
                if let Ok(ResponseBody::WorkbenchGet(snapshot)) = request(
                    &connection,
                    RequestBody::WorkbenchGet(WorkbenchGetRequest {
                        workspace_id: workspace.clone(),
                    }),
                )
                .await
                    && let Ok(mut replica) = replica.try_borrow_mut()
                    && api_workspace(&replica.workspace) == workspace
                    && let Some(state) = versioned_workbench(snapshot)
                {
                    let _ = replica.workbench.refresh(state);
                }
            }
            EventBody::WorkspaceViewChanged {
                workspace_id,
                revision,
                view,
            } => {
                let current = replica.borrow().workspace.clone();
                if current.as_str() == workspace_id.as_str() {
                    super::SESSION_VIEW.with(|session| {
                        session.borrow_mut().apply_event(&current, revision, view);
                    });
                    // タブ構成が変わったことを UI に知らせ、適用させる。
                    notify_session_view(&session_view_changes);
                }
            }
            EventBody::SnapshotRequired { .. } => {
                // イベントを取りこぼした。Host の Snapshot で複製を取り直す。
                // 共有中のタブ構成も snapshot から取り直すまで古い可能性がある。
                super::SESSION_VIEW.with(|session| session.borrow().watch().mark_stale());
                notify_session_view(&session_view_changes);
                let delta = sync_snapshot(&connection, &replica).await;
                if delta.display {
                    notify_changes(&changes);
                }
                if delta.documents {
                    // Editor は DocumentConnection::subscribe を持たないため、
                    // 文書の変更を専用の通知で伝えて再同期させる。
                    notify_documents(&document_changes);
                }
                // 共有 view state を snapshot で戻し、取り直せたら通知する。
                if let Some(state) =
                    super::SESSION_VIEW.with(|session| session.borrow().workspace().cloned())
                {
                    let body = request(
                        &connection,
                        RequestBody::WorkspaceViewSubscribe(WorkspaceViewSubscribeRequest {
                            workspace_id: api_workspace(&state),
                        }),
                    )
                    .await;
                    if let Ok(ResponseBody::WorkspaceViewSubscribe(snapshot)) = body {
                        super::SESSION_VIEW.with(|session| {
                            session.borrow_mut().subscribe_snapshot(
                                state.clone(),
                                snapshot.revision,
                                snapshot.view,
                            );
                        });
                        notify_session_view(&session_view_changes);
                    }
                }
            }
            _ => {}
        }
    }
}

/// `snapshot.required` の後で、保持している複製を Host の Snapshot へ合わせる。
async fn sync_snapshot(
    connection: &BrowserConnection,
    replica: &Rc<RefCell<Replica>>,
) -> crate::web_cache::SnapshotDelta {
    let workspace = api_workspace(&replica.borrow().workspace);
    let Ok(ResponseBody::SnapshotResync(response)) = request(
        connection,
        RequestBody::SnapshotResync(SnapshotRequest {
            workspace_id: workspace.clone(),
            reason: SnapshotReason::EventGap,
        }),
    )
    .await
    else {
        return crate::web_cache::SnapshotDelta::default();
    };
    let Ok(mut replica) = replica.try_borrow_mut() else {
        return crate::web_cache::SnapshotDelta::default();
    };
    if api_workspace(&replica.workspace) != workspace {
        return crate::web_cache::SnapshotDelta::default();
    }
    crate::web_cache::apply_snapshot(&mut replica, &response.snapshot)
}

async fn handle(
    connection: &BrowserConnection,
    replica: &Rc<RefCell<Replica>>,
    changes: &RefCell<Vec<mpsc::UnboundedSender<()>>>,
    document_changes: &RefCell<Vec<mpsc::UnboundedSender<()>>>,
    session_view_changes: &RefCell<Vec<mpsc::UnboundedSender<()>>>,
    command: Command,
) -> Result<(), String> {
    // 送信待ちを流し終えた後は、変更系Commandを成功扱いで捨てず呼び出し元へ返す。
    if super::DRAINING.with(Cell::get) && command.is_mutation() {
        return Err(crate::web_switch::SWITCHING_MESSAGE.into());
    }
    match command {
        Command::Barrier { done } => {
            let _ = done.send(());
        }
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
                state.owner = owner_of(&snapshot);
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
                    refresh_document(connection, replica, &local, true).await?;
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
                Err(_) => refresh_document(connection, replica, &local, true).await?,
            }
        }
        Command::Claim { local } => {
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
                    RequestBody::DocumentClaim(DocumentClaimRequest { document_id: host }),
                )
                .await;
            }
            refresh_document(connection, replica, &local, true).await?;
        }
        Command::Release { local } => {
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
                    RequestBody::DocumentRelease(DocumentReleaseRequest { document_id: host }),
                )
                .await;
            }
            refresh_document(connection, replica, &local, true).await?;
        }
        Command::Reload { local } => {
            refresh_document(connection, replica, &local, false).await?;
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
                refresh_document(connection, replica, &local, false).await?;
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
            profile,
        } => {
            // 切替で消えた旧WorkspaceのTerminalを新Workspaceに作らない。
            if !replica.borrow().terminals.contains_key(local.as_str()) {
                return Err("Terminalがありません".into());
            }
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
                    profile,
                    size: TerminalSize { columns, rows },
                }),
            )
            .await?;
            let ResponseBody::TerminalStart(response) = body else {
                return Err("TerminalStart応答が不正です".into());
            };
            let mut replica = replica.borrow_mut();
            if replica.terminals.contains_key(local.as_str()) {
                let snapshot = model_terminal_snapshot(&local, &response.terminal);
                replica.connect_terminal(&local, response.terminal.terminal_id, snapshot);
            }
            // 応答前のイベントを保留していた場合も、対応付け後の snapshot を UI に届ける。
            drop(replica);
            notify_changes(changes);
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
        Command::TerminalClaim { local } => {
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
                    RequestBody::TerminalClaim(TerminalClaimRequest { terminal_id: host }),
                )
                .await;
            }
        }
        Command::TerminalRelease { local } => {
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
                    RequestBody::TerminalRelease(TerminalReleaseRequest { terminal_id: host }),
                )
                .await;
            }
        }
        // delta 欠番や開始応答前の取りこぼしを、Host の現在の snapshot で回復する。
        Command::ResyncTerminals => {
            let delta = sync_snapshot(connection, replica).await;
            if delta.display {
                notify_changes(changes);
            }
            if delta.documents {
                notify_documents(document_changes);
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
        Command::PublishView { view } => {
            let workspace = replica.borrow().workspace.clone();
            let body = request(
                connection,
                RequestBody::WorkspaceViewUpdate(WorkspaceViewUpdateRequest {
                    workspace_id: api_workspace(&workspace),
                    view,
                }),
            )
            .await?;
            let ResponseBody::WorkspaceViewUpdate(response) = body else {
                return Err("WorkspaceViewUpdate応答が不正です".into());
            };
            // 自分の送信は event で戻ってきても再適用しない。
            super::SESSION_VIEW.with(|session| session.borrow().accept_local(response.revision));
        }
        Command::ResyncView => {
            let workspace = replica.borrow().workspace.clone();
            let body = request(
                connection,
                RequestBody::WorkspaceViewSubscribe(client_api::WorkspaceViewSubscribeRequest {
                    workspace_id: api_workspace(&workspace),
                }),
            )
            .await?;
            let ResponseBody::WorkspaceViewSubscribe(snapshot) = body else {
                return Err("WorkspaceViewSubscribe応答が不正です".into());
            };
            super::SESSION_VIEW.with(|session| {
                session.borrow_mut().subscribe_snapshot(
                    workspace,
                    snapshot.revision,
                    snapshot.view,
                );
            });
            // snapshot を反映したら UI に変更を知らせる。
            notify_session_view(session_view_changes);
        }
        Command::CloseView => {
            let workspace = replica.borrow().workspace.clone();
            let _ = request(
                connection,
                RequestBody::WorkspaceViewUnsubscribe(
                    client_api::WorkspaceViewUnsubscribeRequest {
                        workspace_id: api_workspace(&workspace),
                    },
                ),
            )
            .await;
            super::SESSION_VIEW.with(|session| session.borrow_mut().reset());
            notify_session_view(session_view_changes);
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
    preserve_local: bool,
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
        crate::web_cache::apply_inspection(state, &snapshot, preserve_local);
    }
    Ok(())
}
