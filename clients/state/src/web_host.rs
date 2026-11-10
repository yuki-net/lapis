//! Hostへ接続し、GPUI UIの同期Clientへローカル複製を提供する。
//!
//! 読み取りは複製から即座に返し、書き込みは複製へ反映してHostへ非同期送信する。
//! Hostが拒否した場合は正本を取得し直し、次のsnapshotでUIへ反映する。

use std::{
    cell::RefCell,
    ops::Range,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

use crate::{
    ConversationRecord, Project, WorkspaceDialog, WorkspaceSnapshot, WorkspaceStateRepository,
};
use client_api::{
    ClientKind, RequestBody, ResponseBody, Revision as ApiRevision, SnapshotReason,
    SnapshotRequest, WorkbenchGetRequest, WorkspaceConnectRequest, WorkspaceId as ApiWorkspaceId,
    WorkspaceListRequest, WorkspaceRelativePath,
};
use client_core::browser::{WebClientConnectOptions, connect_web_client};
use document::{
    Document, DocumentCache, DocumentConnection, DocumentError, Encoding, ExternalChange, Revision,
};
use editor_core::{DocumentId, SessionId, WorkspaceId};
use futures_channel::mpsc;
use git::{GitConnection, GitDiffSnapshot, GitError, GitErrorKind, GitSnapshot, WorktreeSnapshot};
use lsp::{
    CompletionItem, DefinitionTarget, Diagnostic, LanguageServerBackend, LspError, LspPosition,
};
use settings::{GlobalSettings, GlobalSettingsRepository, SettingsError};
use task::{TaskConnection, TaskControl, TaskError, TaskSnapshot, TaskStart};
use terminal::{
    TerminalBackend, TerminalError, TerminalEvent, TerminalId, TerminalInput, TerminalSnapshot,
};
use workbench::ConversationViewState;
use workspace::{
    FileEntry, SearchError, SearchHit, WorkspaceError, WorkspaceRepository, WorkspaceSearchBackend,
};

mod state;
mod transport;

use state::{
    DocumentState, Replica, TerminalState, cache_of, local_id, model_terminal_snapshot,
    versioned_workbench,
};
use transport::{Command, event_pump, fetch_tree, preload, preload_workbench, pump, request};

pub const ROOT: &str = r"C:\Lapis Host";
const UNAVAILABLE: &str = "Webではこの機能に接続していません";
const MAX_TREE_ENTRIES: usize = 5000;
const MAX_PRELOAD_BYTES: usize = 2 * 1024 * 1024;
const WEB_CAPABILITIES: &[&str] = &[
    "workspace.list",
    "workspace.connect",
    "workspace.switch",
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
    changes_rx: Option<mpsc::UnboundedReceiver<()>>,
}

/// Host接続の設定。ページのクエリかlocalStorageから渡される。
pub struct HostConfig {
    pub url: String,
    pub workspace: String,
    pub client_id: String,
    pub token: Option<String>,
    pub credential: Option<String>,
    pub id_token: Option<String>,
}

/// Hostへ接続し、初期状態を取得してバックエンドを設置する。pairing時はcredentialを返す。
pub async fn connect(config: HostConfig) -> Result<Option<String>, String> {
    let requested_workspace = if config.workspace.trim().is_empty() {
        None
    } else {
        Some(
            ApiWorkspaceId::try_new(config.workspace.clone())
                .map_err(|_| "Workspace IDが不正です")?,
        )
    };
    let (connection, fresh_credential, selected_workspace) =
        connect_web_client(WebClientConnectOptions {
            url: &config.url,
            workspace: requested_workspace.clone(),
            client_id: &config.client_id,
            client_name: "lapis-web",
            client_kind: ClientKind::Desktop,
            token: config.token.as_deref(),
            credential: config.credential.as_deref(),
            id_token: config.id_token.as_deref(),
            capabilities: WEB_CAPABILITIES,
        })
        .await?;
    let mut connection = Rc::new(connection);
    let api_workspace = match selected_workspace.or(requested_workspace) {
        Some(workspace) => workspace,
        None => {
            // credential が Workspace に固定されていない場合は Host の一覧から選ぶ。
            let ResponseBody::WorkspaceList(list) = request(
                &connection,
                RequestBody::WorkspaceList(WorkspaceListRequest::default()),
            )
            .await?
            else {
                return Err("Workspace一覧を取得できません".to_owned());
            };
            list.workspaces
                .first()
                .map(|workspace| workspace.workspace_id.clone())
                .ok_or_else(|| "HostにWorkspaceが登録されていません".to_owned())?
        }
    };
    let workspace = WorkspaceId::new(api_workspace.as_str());
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
    let replica = Rc::new(RefCell::new(Replica::new(
        workspace.clone(),
        entries,
        connection
            .session_id()
            .map(|session| SessionId::new(session.as_str())),
    )));
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
    let (changes, changes_rx) = mpsc::unbounded();
    BACKEND.with(|backend| {
        *backend.borrow_mut() = Some(BackendState {
            workspace,
            replica: replica.clone(),
            commands,
            changes_rx: Some(changes_rx),
        })
    });
    wasm_bindgen_futures::spawn_local(pump(connection.clone(), receiver, replica.clone()));
    if let Some(events) = events {
        wasm_bindgen_futures::spawn_local(event_pump(
            connection.clone(),
            events,
            replica.clone(),
            changes,
        ));
    }
    preload(&connection, &replica).await?;
    Ok(fresh_credential)
}

pub(super) fn api_workspace(id: &WorkspaceId) -> ApiWorkspaceId {
    ApiWorkspaceId::try_from(id).expect("workspace id is valid")
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
            let session = replica.session.clone();
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
                owner: None,
            };
            let cache = cache_of(
                &local,
                &state.document,
                state.external_change,
                state.owner.as_ref(),
                session.as_ref(),
            );
            replica.documents.insert(local, state);
            cache
        })
        .map_err(DocumentError::io)
    }

    fn open(&self, path: &Path) -> Result<DocumentCache, DocumentError> {
        self.with(|state| {
            let mut replica = state.replica.borrow_mut();
            let session = replica.session.clone();
            if let Some(local) = replica.by_path.get(path).cloned() {
                let state = replica
                    .documents
                    .get(&local)
                    .ok_or_else(|| DocumentError::io("Documentがありません"))?;
                return Ok(cache_of(
                    &local,
                    &state.document,
                    state.external_change,
                    state.owner.as_ref(),
                    session.as_ref(),
                ));
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
                owner: None,
            };
            let cache = cache_of(
                &local,
                &document_state.document,
                document_state.external_change,
                document_state.owner.as_ref(),
                session.as_ref(),
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
            let session = replica.session.clone();
            let document = replica
                .documents
                .get(id)
                .ok_or_else(|| DocumentError::io("Documentがありません"))?;
            Ok(cache_of(
                id,
                &document.document,
                document.external_change,
                document.owner.as_ref(),
                session.as_ref(),
            ))
        })
        .map_err(DocumentError::io)?
    }

    fn claim(&self, id: &DocumentId) -> Result<DocumentCache, DocumentError> {
        self.with(|state| {
            let mut replica = state.replica.borrow_mut();
            let session = replica
                .session
                .clone()
                .ok_or_else(|| DocumentError::io("Sessionが不明です"))?;
            let document = replica
                .documents
                .get_mut(id)
                .ok_or_else(|| DocumentError::io("Documentがありません"))?;
            if document.host.is_none() {
                return Err(DocumentError::io("HostにDocumentがありません"));
            }
            document.owner = Some(session.clone());
            let cache = cache_of(
                id,
                &document.document,
                document.external_change,
                document.owner.as_ref(),
                Some(&session),
            );
            let _ = state
                .commands
                .unbounded_send(Command::Claim { local: id.clone() });
            Ok(cache)
        })
        .map_err(DocumentError::io)?
    }

    fn release(&self, id: &DocumentId) -> Result<DocumentCache, DocumentError> {
        self.with(|state| {
            let mut replica = state.replica.borrow_mut();
            let session = replica.session.clone();
            let document = replica
                .documents
                .get_mut(id)
                .ok_or_else(|| DocumentError::io("Documentがありません"))?;
            if document
                .owner
                .as_ref()
                .is_some_and(|owner| session.as_ref().is_none_or(|session| owner != session))
            {
                return Err(DocumentError::conflict("他のClientが編集中です"));
            }
            document.owner = None;
            let cache = cache_of(
                id,
                &document.document,
                document.external_change,
                document.owner.as_ref(),
                session.as_ref(),
            );
            let _ = state
                .commands
                .unbounded_send(Command::Release { local: id.clone() });
            Ok(cache)
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
            let session = replica.session.clone();
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
            let cache = cache_of(
                id,
                &document.document,
                document.external_change,
                document.owner.as_ref(),
                session.as_ref(),
            );
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
            let session = replica.session.clone();
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
            let cache = cache_of(
                id,
                &document.document,
                document.external_change,
                document.owner.as_ref(),
                session.as_ref(),
            );
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
            let session = replica.session.clone();
            let document = replica
                .documents
                .get_mut(id)
                .ok_or_else(|| DocumentError::io("Documentがありません"))?;
            if document.document.revision() != revision {
                return Err(DocumentError::conflict(
                    "Documentのrevisionが競合しています",
                ));
            }
            let cache = cache_of(
                id,
                &document.document,
                document.external_change,
                document.owner.as_ref(),
                session.as_ref(),
            );
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
            let session = replica.session.clone();
            let document = replica
                .documents
                .get_mut(id)
                .ok_or_else(|| DocumentError::io("Documentがありません"))?;
            if document.document.revision() != revision {
                return Err(DocumentError::conflict(
                    "Documentのrevisionが競合しています",
                ));
            }
            let cache = cache_of(
                id,
                &document.document,
                document.external_change,
                document.owner.as_ref(),
                session.as_ref(),
            );
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
            let session = replica.session.clone();
            let document = replica
                .documents
                .get_mut(id)
                .ok_or_else(|| DocumentError::io("Documentがありません"))?;
            if document.document.revision() != revision {
                return Err(DocumentError::conflict(
                    "Documentのrevisionが競合しています",
                ));
            }
            let cache = cache_of(
                id,
                &document.document,
                document.external_change,
                document.owner.as_ref(),
                session.as_ref(),
            );
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
    shared: &workbench::SharedWorkspaceView,
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
) -> Result<workbench::SharedWorkspaceView, WorkspaceError> {
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
            Ok(workbench::SharedDocumentView {
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
    Ok(workbench::SharedWorkspaceView {
        workspace_id,
        open_documents: documents,
        active_path: snapshot.active_path.as_deref().map(relative).transpose()?,
        active_document_id: snapshot.active_document_id.clone(),
    })
}

/// 接続時に取得したHostのWorkbenchから、UIへ復元するレイアウト。
pub fn restore_view() -> Option<workbench::ConversationViewState> {
    BACKEND.with(|backend| {
        let backend = backend.borrow();
        let state = backend.as_ref()?;
        let replica = state.replica.borrow();
        let view = replica
            .workbench
            .confirmed()
            .and_then(|versioned| versioned.view.as_ref())?
            .clone();
        Some(workbench::ConversationViewState {
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
            let id = editor_core::ConversationId::new("web-conversation");
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
        active: &editor_core::ConversationId,
    ) -> Result<(), WorkspaceError> {
        self.with(|state| {
            let record = records
                .iter()
                .find(|record| &record.id == active)
                .or_else(|| records.first())
                .ok_or_else(|| WorkspaceError::new("保存するConversationがありません"))?;
            let replica = state.replica.borrow();
            if replica.workbench.confirmed().is_none() {
                return Err(WorkspaceError::new("Workbenchが初期化されていません"));
            }
            if replica.workbench.conflict().is_some() {
                return Err(WorkspaceError::new(
                    "Workbenchが競合しています。Hostの状態を採用するか、ローカルの変更を再送してください",
                ));
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

    fn accept_workbench_remote(&self) -> Result<Option<ConversationViewState>, WorkspaceError> {
        self.with(|state| {
            let state = state
                .replica
                .borrow_mut()
                .workbench
                .accept_remote()
                .map_err(WorkspaceError::new)?;
            let mut view = state
                .view
                .ok_or_else(|| WorkspaceError::new("HostのWorkbench状態が不正です"))?;
            view.workspace = None;
            Ok(Some(view))
        })
        .map_err(WorkspaceError::new)?
    }

    fn retry_workbench_local(&self) -> Result<(), WorkspaceError> {
        self.with(|state| {
            state
                .commands
                .unbounded_send(Command::RetryWorkbenchLocal)
                .map_err(|_| WorkspaceError::new("Hostのコマンドを送信できません"))
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
        _: &editor_core::ExecutionId,
        _: &TaskControl,
    ) -> Result<(), TaskError> {
        Err(TaskError::new(UNAVAILABLE))
    }
}

impl GitConnection for WebBackend {
    fn status(
        &self,
        _: &WorkspaceId,
        _: Option<&editor_core::TaskId>,
    ) -> Result<GitSnapshot, GitError> {
        Err(GitError::new(GitErrorKind::Io, UNAVAILABLE))
    }
    fn diff(
        &self,
        _: &WorkspaceId,
        _: Option<&editor_core::TaskId>,
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
        _: &editor_core::TaskId,
        _: &str,
    ) -> Result<WorktreeSnapshot, GitError> {
        Err(GitError::new(GitErrorKind::Io, UNAVAILABLE))
    }
    fn discard_worktree(
        &self,
        _: &WorkspaceId,
        _: &editor_core::TaskId,
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
        self.start_with_shell(cwd, columns, rows, terminal::TerminalShell::Default)
    }

    fn start_with_shell(
        &self,
        cwd: &Path,
        columns: u16,
        rows: u16,
        shell: terminal::TerminalShell,
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

    fn resize(&self, id: &TerminalId, columns: u16, rows: u16) -> Result<(), TerminalError> {
        let sent = self
            .with(|state| {
                // 画面は terminal.delta イベントで届くため、ここでは要求だけ送る。
                state
                    .commands
                    .unbounded_send(Command::TerminalResize {
                        local: id.clone(),
                        columns,
                        rows,
                    })
                    .is_ok()
            })
            .map_err(TerminalError::new)?;
        if !sent {
            return Err(TerminalError::new("Terminalの要求を送信できません"));
        }
        Ok(())
    }

    fn claim(&self, id: &TerminalId) -> Result<(), TerminalError> {
        let sent = self
            .with(|state| {
                state
                    .commands
                    .unbounded_send(Command::TerminalClaim { local: id.clone() })
                    .is_ok()
            })
            .map_err(TerminalError::new)?;
        if !sent {
            return Err(TerminalError::new("Terminalの要求を送信できません"));
        }
        Ok(())
    }

    fn release(&self, id: &TerminalId) -> Result<(), TerminalError> {
        let sent = self
            .with(|state| {
                state
                    .commands
                    .unbounded_send(Command::TerminalRelease { local: id.clone() })
                    .is_ok()
            })
            .map_err(TerminalError::new)?;
        if !sent {
            return Err(TerminalError::new("Terminalの要求を送信できません"));
        }
        Ok(())
    }

    fn changes(&self) -> Option<mpsc::UnboundedReceiver<()>> {
        self.with(|state| state.changes_rx.take()).ok().flatten()
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
