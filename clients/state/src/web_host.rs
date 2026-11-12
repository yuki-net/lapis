//! Hostへ接続し、GPUI UIの同期Clientへローカル複製を提供する。
//!
//! 読み取りは複製から即座に返し、書き込みは複製へ反映してHostへ非同期送信する。
//! Hostが拒否した場合は正本を取得し直し、次のsnapshotでUIへ反映する。

use std::{
    cell::{Cell, RefCell},
    ops::Range,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

use crate::{
    ConversationRecord, Project, SessionViewReplica, SessionViewWatch, WorkspaceDialog,
    WorkspaceStateRepository, WorkspaceViewState,
};
use client_api::{
    ClientKind, DocumentInspectRequest, RequestBody, ResponseBody, Revision as ApiRevision,
    SnapshotReason, SnapshotRequest, TerminalListProfilesRequest, WorkbenchGetRequest,
    WorkspaceCloseRequest, WorkspaceConnectRequest, WorkspaceId as ApiWorkspaceId,
    WorkspaceListRequest, WorkspaceRelativePath, WorkspaceSummary, WorkspaceViewUnsubscribeRequest,
};
use client_core::browser::{BrowserConnection, WebClientConnectOptions, connect_web_client};
use document::{
    Document, DocumentCache, DocumentConnection, DocumentError, Encoding, ExternalChange, Revision,
};
use editor_core::{DocumentId, SessionId, WorkspaceId};
use futures_channel::{mpsc, oneshot};
use git::{GitConnection, GitDiffSnapshot, GitError, GitErrorKind, GitSnapshot, WorktreeSnapshot};
use lsp::{
    CompletionItem, DefinitionTarget, Diagnostic, LanguageServerBackend, LspError, LspPosition,
};
use settings::{SettingsDocument, SettingsError, SettingsRepository, SettingsScope};
use task::{TaskConnection, TaskControl, TaskError, TaskSnapshot, TaskStart};
use terminal::{
    TerminalBackend, TerminalError, TerminalEvent, TerminalId, TerminalInput, TerminalSnapshot,
};
use workbench::ConversationViewState;
use workspace::{
    FileEntry, SearchError, SearchHit, WorkspaceError, WorkspaceIdentity, WorkspaceRepository,
    WorkspaceSearchBackend,
};

mod transport;

pub use crate::web_replica::ROOT;

use crate::web_cache::{CachedWorkspace, WorkspaceCache};
use crate::web_replica::{
    DocumentState, Replica, TerminalState, cache_of, local_id, terminal_state, versioned_workbench,
};
use crate::web_switch::{self, Command, Operation};
use transport::{event_pump, fetch_tree, preload, preload_workbench, pump, request};

const UNAVAILABLE: &str = "Webではこの機能に接続していません";
const MAX_TREE_ENTRIES: usize = 5000;
const MAX_PRELOAD_BYTES: usize = 2 * 1024 * 1024;

thread_local! {
    static BACKEND: RefCell<Option<BackendState>> = const { RefCell::new(None) };
    /// 同時に複数の切替を開始しない。
    static SWITCHING: Cell<bool> = const { Cell::new(false) };
    /// 送信待ちのCommandを流し終えた後、変更系Commandを止める。
    static DRAINING: Cell<bool> = const { Cell::new(false) };
    /// Root の再構築が終わるまで保持する切替前の複製。失敗時に元へ戻す。
    static PREVIOUS: RefCell<Option<PreviousWorkspace>> = const { RefCell::new(None) };
    /// ページの生存中だけ保持する Workspace ごとの複製。永続化はしない。
    static CACHE: RefCell<WorkspaceCache> = const { RefCell::new(WorkspaceCache::new(0)) };
    /// Workspace の view state 共有。UI と同じ watch を共有する。
    static SESSION_VIEW: RefCell<SessionViewReplica> =
        RefCell::new(SessionViewReplica::new(SessionViewWatch::new()));
}

struct PreviousWorkspace {
    workspace: WorkspaceId,
    workspace_identity: WorkspaceIdentity,
    replica: Replica,
    /// 切替先が保持済みの複製から復元されたか。rollback で切替先を保持へ戻す。
    target_was_cached: bool,
}

struct BackendState {
    connection: Rc<BrowserConnection>,
    workspace: WorkspaceId,
    workspace_identity: WorkspaceIdentity,
    host_name: String,
    host_id: Option<String>,
    can_switch: bool,
    workspaces: Vec<WorkspaceSummary>,
    replica: Rc<RefCell<Replica>>,
    commands: mpsc::UnboundedSender<Command>,
    changes: Rc<RefCell<Vec<mpsc::UnboundedSender<()>>>>,
    /// Editor 向けの文書変更通知。Terminal 向けの `changes` とは分ける。
    document_changes: Rc<RefCell<Vec<mpsc::UnboundedSender<()>>>>,
    /// 共有 view state の変更通知。tabs の適用は他の通知と独立させる。
    session_view_changes: Rc<RefCell<Vec<mpsc::UnboundedSender<()>>>>,
}

/// Web UI が Host/Workspace の識別子と切り替え可否を表示するための情報。
#[derive(serde::Serialize)]
pub struct ConnectionInfo {
    pub host_name: String,
    pub host_id: Option<String>,
    pub workspace_name: String,
    pub workspace_id: String,
    pub can_switch: bool,
    pub workspaces: Vec<WorkspaceChoice>,
}

#[derive(serde::Serialize)]
pub struct WorkspaceChoice {
    pub id: String,
    pub name: String,
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
    let host_name = crate::host_name_from_url(&config.url);
    let requested_workspace = if config.workspace.trim().is_empty() {
        None
    } else {
        Some(
            ApiWorkspaceId::try_new(config.workspace.clone())
                .map_err(|_| "Workspace IDが不正です")?,
        )
    };
    let connected = connect_web_client(WebClientConnectOptions {
        url: &config.url,
        workspace: requested_workspace.clone(),
        client_id: &config.client_id,
        client_name: "lapis-web",
        client_kind: ClientKind::Desktop,
        token: config.token.as_deref(),
        credential: config.credential.as_deref(),
        id_token: config.id_token.as_deref(),
        capabilities: client_core::web_capabilities::CAPABILITIES,
    })
    .await?;
    let mut connection = Rc::new(connected.connection);
    let ResponseBody::WorkspaceList(workspace_list) = request(
        &connection,
        RequestBody::WorkspaceList(WorkspaceListRequest::default()),
    )
    .await?
    else {
        return Err("Workspace一覧を取得できません".to_owned());
    };
    let api_workspace = match connected.workspace_id.or(requested_workspace) {
        Some(workspace) => workspace,
        None => {
            // credential が Workspace に固定されていない場合は Host の一覧から選ぶ。
            workspace_list
                .workspaces
                .first()
                .map(|workspace| workspace.workspace_id.clone())
                .ok_or_else(|| "HostにWorkspaceが登録されていません".to_owned())?
        }
    };
    let can_switch = connected
        .capabilities
        .iter()
        .any(|capability| capability.as_str() == client_api::capability::WORKSPACES_SWITCH);
    let host_id = connected.host_id.clone();
    let fresh_credential = connected.credential.clone();
    let workspaces = workspace_list.workspaces;
    // 保持の上限は、接続先が持つ Workspace の数とする。active は含めない。
    CACHE.with(|cache| *cache.borrow_mut() = WorkspaceCache::new(workspaces.len()));
    let workspace = WorkspaceId::new(api_workspace.as_str());
    let events = Rc::get_mut(&mut connection).and_then(|connection| connection.take_events());
    let workspace_identity = match request(
        &connection,
        RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
            workspace_id: api_workspace.clone(),
        }),
    )
    .await?
    {
        ResponseBody::WorkspaceConnect(response) => WorkspaceIdentity {
            id: workspace.clone(),
            name: response.identity.name,
            root_name: response.identity.root_name,
        },
        ResponseBody::Error(error) => return Err(error.code.as_str().to_owned()),
        _ => return Err("Workspace identityを取得できません".to_owned()),
    };
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
    let profiles = match request(
        &connection,
        RequestBody::TerminalListProfiles(TerminalListProfilesRequest {
            workspace_id: api_workspace.clone(),
        }),
    )
    .await
    {
        Ok(ResponseBody::TerminalListProfiles(response)) => response.profiles,
        _ => terminal::TerminalProfiles::default(),
    };
    let replica = Rc::new(RefCell::new(Replica::new(
        workspace.clone(),
        entries,
        connection
            .session_id()
            .map(|session| SessionId::new(session.as_str())),
        profiles,
    )));
    {
        let mut replica = replica.borrow_mut();
        for terminal in terminals {
            let local = TerminalId::new(format!("web-terminal-{}", replica.next_terminal));
            replica.next_terminal += 1;
            let state = terminal_state(&local, &terminal);
            replica.terminals.insert(local.as_str().to_owned(), state);
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
    let changes = Rc::new(RefCell::new(Vec::new()));
    let document_changes = Rc::new(RefCell::new(Vec::new()));
    let session_view_changes = Rc::new(RefCell::new(Vec::new()));
    BACKEND.with(|backend| {
        *backend.borrow_mut() = Some(BackendState {
            connection: connection.clone(),
            workspace: workspace.clone(),
            workspace_identity,
            host_name,
            host_id,
            can_switch,
            workspaces,
            replica: replica.clone(),
            commands: commands.clone(),
            changes: changes.clone(),
            document_changes: document_changes.clone(),
            session_view_changes: session_view_changes.clone(),
        })
    });
    wasm_bindgen_futures::spawn_local(pump(
        connection.clone(),
        receiver,
        replica.clone(),
        changes.clone(),
        document_changes.clone(),
        session_view_changes.clone(),
    ));
    if let Some(events) = events {
        wasm_bindgen_futures::spawn_local(event_pump(
            connection.clone(),
            events,
            replica.clone(),
            changes,
            document_changes,
            session_view_changes,
        ));
    }
    preload(&connection, &replica).await?;
    // 購読は接続直後から始め、snapshot を現在のタブ構成へ適用できるようにする。
    resync_session_view();
    Ok(fresh_credential)
}

/// 同一接続のまま別の Workspace へ切り替える。
///
/// 接続中の Workspace は `workspace.close` で attachment だけを解除し、
/// 同じ Session のまま `workspace.connect` で切り替え先へ接続する。
/// ページ遷移や再接続は行わない。
pub async fn switch_workspace(workspace_id: &str) -> Result<(), String> {
    let requested =
        ApiWorkspaceId::try_new(workspace_id.to_owned()).map_err(|_| "Workspace IDが不正です")?;
    let (connection, current, can_switch, commands) = BACKEND.with(|backend| {
        let backend = backend.borrow();
        let state = backend.as_ref().ok_or("Hostに接続していません")?;
        Ok::<_, String>((
            state.connection.clone(),
            state.workspace.clone(),
            state.can_switch,
            state.commands.clone(),
        ))
    })?;
    if !can_switch {
        return Err("この接続ではWorkspaceを切り替えられません".to_owned());
    }
    if current.as_str() == requested.as_str() {
        return Ok(());
    }
    if SWITCHING.with(|switching| switching.replace(true)) {
        return Err("Workspaceを切り替えています".to_owned());
    }
    // 前回の切替が残した複製も、次の切替を始める前に保持へ移す。
    PREVIOUS.with(|previous| {
        if let Some(previous) = previous.borrow_mut().take() {
            cache_workspace(CachedWorkspace {
                replica: previous.replica,
            });
        }
    });
    // 切替前の編集・保存要求を旧 Workspace へ送り切ってから attachment を閉じる。
    let (done, drained) = oneshot::channel();
    if commands.unbounded_send(Command::Barrier { done }).is_err() {
        SWITCHING.with(|switching| switching.set(false));
        return Err("Workspace切替の待機を開始できません".to_owned());
    }
    if drained.await.is_err() {
        SWITCHING.with(|switching| switching.set(false));
        return Err("Workspace切替の待機に失敗しました".to_owned());
    }
    DRAINING.with(|draining| draining.set(true));
    let result = switch_workspace_inner(&connection, &current, &requested).await;
    DRAINING.with(|draining| draining.set(false));
    SWITCHING.with(|switching| switching.set(false));
    result
}

async fn switch_workspace_inner(
    connection: &BrowserConnection,
    current: &WorkspaceId,
    requested: &ApiWorkspaceId,
) -> Result<(), String> {
    let current_api = api_workspace(current);
    // 離れる Workspace の共有購読を解除する。閉じたら以後 delta は届かない。
    let _ = request(
        connection,
        RequestBody::WorkspaceViewUnsubscribe(WorkspaceViewUnsubscribeRequest {
            workspace_id: current_api.clone(),
        }),
    )
    .await;
    SESSION_VIEW.with(|session| session.borrow_mut().reset());
    request(
        connection,
        RequestBody::WorkspaceClose(WorkspaceCloseRequest {
            workspace_id: current_api.clone(),
        }),
    )
    .await?;
    let workspace_identity = match request(
        connection,
        RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
            workspace_id: requested.clone(),
        }),
    )
    .await
    {
        Ok(ResponseBody::WorkspaceConnect(response)) => WorkspaceIdentity {
            id: WorkspaceId::new(requested.as_str()),
            name: response.identity.name,
            root_name: response.identity.root_name,
        },
        Ok(ResponseBody::Error(error)) => {
            return Err(fail_switch(
                connection,
                &current_api,
                None,
                error.code.as_str().to_owned(),
            )
            .await);
        }
        Ok(_) => {
            return Err(fail_switch(
                connection,
                &current_api,
                None,
                "Workspace identityを取得できません".to_owned(),
            )
            .await);
        }
        Err(error) => {
            return Err(fail_switch(connection, &current_api, None, error).await);
        }
    };
    let workspace = WorkspaceId::new(requested.as_str());

    // 保持済みの複製があれば表示状態をそのまま復元し、離れていた間の差分だけを取り直す。
    if let Some(cached) = CACHE.with(|cache| cache.borrow_mut().take(&workspace)) {
        let mut replica = cached.replica;
        // 復元した複製を先に最新化してから設置する。切替中に旧 Workspace の
        // 読み取りが新しい複製の同じ local ID を参照しないようにする。
        if let Err(error) = resync_workspace(connection, &mut replica, &workspace).await {
            // 再同期できない複製は設置せず、保持へ戻して元の Workspace へ復旧する。
            cache_workspace(CachedWorkspace { replica });
            return Err(fail_switch(connection, &current_api, Some(requested), error).await);
        }
        install_workspace(current, workspace, workspace_identity, replica, true)?;
        resync_session_view();
        return Ok(());
    }

    let entries = match fetch_tree(connection, requested).await {
        Ok(entries) => entries,
        Err(error) => {
            return Err(fail_switch(connection, &current_api, Some(requested), error).await);
        }
    };
    let terminals = match request(
        connection,
        RequestBody::SnapshotResync(SnapshotRequest {
            workspace_id: requested.clone(),
            reason: SnapshotReason::Reconnect,
        }),
    )
    .await
    {
        Ok(ResponseBody::SnapshotResync(response)) => response.snapshot.terminals,
        _ => Vec::new(),
    };
    let profiles = match request(
        connection,
        RequestBody::TerminalListProfiles(TerminalListProfilesRequest {
            workspace_id: requested.clone(),
        }),
    )
    .await
    {
        Ok(ResponseBody::TerminalListProfiles(response)) => response.profiles,
        _ => terminal::TerminalProfiles::default(),
    };
    let workbench = match request(
        connection,
        RequestBody::WorkbenchGet(WorkbenchGetRequest {
            workspace_id: requested.clone(),
        }),
    )
    .await
    {
        Ok(ResponseBody::WorkbenchGet(snapshot)) => versioned_workbench(snapshot),
        _ => None,
    };
    let replica = BACKEND
        .with(|backend| backend.borrow().as_ref().map(|state| state.replica.clone()))
        .ok_or("Hostに接続していません")?;
    // 表示中の Root を組み直せなかった場合に戻せるよう、切替前の複製を残す。
    let (next, next_terminal, session) = {
        let replica = replica.borrow();
        (replica.next, replica.next_terminal, replica.session.clone())
    };
    let mut next_replica = Replica::new(workspace.clone(), entries, session, profiles);
    next_replica.next = next;
    next_replica.next_terminal = next_terminal;
    next_replica.workbench = workbench::WorkbenchReplica::default();
    if let Some(state) = workbench {
        let _ = next_replica.workbench.open(state);
    }
    for terminal in terminals {
        let local = TerminalId::new(format!("web-terminal-{}", next_replica.next_terminal));
        next_replica.next_terminal += 1;
        let state = terminal_state(&local, &terminal);
        next_replica
            .terminals
            .insert(local.as_str().to_owned(), state);
    }
    let replica = install_workspace(current, workspace, workspace_identity, next_replica, false)?;
    preload_workbench(connection, &replica, requested).await;
    if let Err(error) = preload(connection, &replica).await {
        web_sys::console::warn_1(
            &format!("Workspace切替後の初期ファイルを開けません: {error}").into(),
        );
    }
    resync_session_view();
    Ok(())
}

/// 切替先の複製を Backend へ設置し、切替前の複製を rollback 用に退避する。
fn install_workspace(
    current: &WorkspaceId,
    workspace: WorkspaceId,
    workspace_identity: WorkspaceIdentity,
    next_replica: Replica,
    target_was_cached: bool,
) -> Result<Rc<RefCell<Replica>>, String> {
    let (replica, previous_identity, previous_replica) = BACKEND.with(|backend| {
        let mut backend = backend.borrow_mut();
        let state = backend.as_mut().ok_or("Hostに接続していません")?;
        let previous_replica = std::mem::replace(&mut *state.replica.borrow_mut(), next_replica);
        let previous_identity =
            std::mem::replace(&mut state.workspace_identity, workspace_identity);
        state.workspace = workspace;
        Ok::<_, String>((state.replica.clone(), previous_identity, previous_replica))
    })?;
    PREVIOUS.with(|previous| {
        *previous.borrow_mut() = Some(PreviousWorkspace {
            workspace: current.clone(),
            workspace_identity: previous_identity,
            replica: previous_replica,
            target_was_cached,
        });
    });
    Ok(replica)
}

/// 保持から復元した Workspace を、既存の再同期機構で差分だけ最新にする。
async fn resync_workspace(
    connection: &BrowserConnection,
    replica: &mut Replica,
    workspace: &WorkspaceId,
) -> Result<(), String> {
    let workspace_api = api_workspace(workspace);
    match request(
        connection,
        RequestBody::SnapshotResync(SnapshotRequest {
            workspace_id: workspace_api.clone(),
            reason: SnapshotReason::Reconnect,
        }),
    )
    .await
    {
        Ok(ResponseBody::SnapshotResync(response)) => {
            if &replica.workspace == workspace {
                let _ = crate::web_cache::apply_snapshot(replica, &response.snapshot);
            }
        }
        Ok(_) => {
            return Err("Workspace復帰後の再同期応答が不正です".to_owned());
        }
        Err(error) => {
            return Err(format!("Workspace復帰後の再同期に失敗しました: {error}"));
        }
    }
    attach_cached_documents(connection, replica).await?;
    match request(
        connection,
        RequestBody::WorkbenchGet(WorkbenchGetRequest {
            workspace_id: workspace_api,
        }),
    )
    .await
    {
        Ok(ResponseBody::WorkbenchGet(snapshot)) => {
            if &replica.workspace == workspace {
                let state = versioned_workbench(snapshot)
                    .ok_or_else(|| "Workspace復帰後のWorkbench状態が不正です".to_owned())?;
                replica.workbench.refresh(state).map_err(|error| {
                    format!("Workspace復帰後のWorkbench再同期に失敗しました: {error}")
                })?;
            }
        }
        Ok(_) => return Err("Workspace復帰後のWorkbench応答が不正です".to_owned()),
        Err(error) => {
            return Err(format!(
                "Workspace復帰後のWorkbench再同期に失敗しました: {error}"
            ));
        }
    }
    Ok(())
}

/// 保持していた Document を Host へ再アタッチする。
///
/// Workspace を離れると attachment は解除される。snapshot.resync は attachment を
/// 作らないため、復帰後に編集・保存できるよう document.inspect で確立する。
async fn attach_cached_documents(
    connection: &BrowserConnection,
    replica: &mut Replica,
) -> Result<(), String> {
    let hosts = replica
        .documents
        .values()
        .filter_map(|state| state.host.clone())
        .collect::<Vec<_>>();
    for host in hosts {
        match request(
            connection,
            RequestBody::DocumentInspect(DocumentInspectRequest {
                document_id: host.clone(),
            }),
        )
        .await
        {
            Ok(ResponseBody::DocumentInspect(response)) => {
                if let Some(state) = replica
                    .documents
                    .values_mut()
                    .find(|state| state.host.as_ref() == Some(&host))
                {
                    let _ = crate::web_cache::apply_inspection(state, &response.document, true);
                }
            }
            // Host から消えた Document は外部削除として残し、切替自体は継続する。
            Err(error) if error == client_api::NOT_FOUND => {
                if let Some(state) = replica
                    .documents
                    .values_mut()
                    .find(|state| state.host.as_ref() == Some(&host))
                {
                    state.external_change = ExternalChange::Deleted;
                }
            }
            Ok(_) => {
                return Err("Workspace復帰後のDocument再アタッチ応答が不正です".to_owned());
            }
            Err(error) => {
                return Err(format!(
                    "Workspace復帰後のDocument再アタッチに失敗しました: {error}"
                ));
            }
        }
    }
    Ok(())
}

/// 複製を保持へ移す。上限を超えた分は古いものから破棄する。
fn cache_workspace(cached: CachedWorkspace) {
    CACHE.with(|cache| {
        if let Some(evicted) = cache.borrow_mut().insert(cached) {
            web_sys::console::log_1(
                &format!(
                    "Workspace状態の保持上限を超えたため破棄しました: {}",
                    evicted.replica.workspace
                )
                .into(),
            );
        }
    });
}

/// 切替先への接続に失敗したとき、開いたままの切替先を閉じて元の Workspace へ戻す。
///
/// `attempted` は接続に成功した切替先だけを渡す。接続自体に失敗した場合は
/// 切替先が開いていないため、閉じずに元の Workspace へ接続し直す。
async fn restore_connection(
    connection: &BrowserConnection,
    current: &ApiWorkspaceId,
    attempted: Option<&ApiWorkspaceId>,
) -> Result<(), String> {
    for step in web_switch::rollback_steps(current, attempted) {
        let body = request(connection, web_switch::attachment_request(&step))
            .await
            .map_err(|error| match &step {
                web_switch::AttachmentStep::Close(_) => {
                    format!("接続中のWorkspaceを閉じられません: {error}")
                }
                web_switch::AttachmentStep::Connect(_) => {
                    format!("元のWorkspaceへ接続できません: {error}")
                }
            })?;
        if matches!(step, web_switch::AttachmentStep::Connect(_)) {
            web_switch::ensure_connected(body)?;
        }
    }
    Ok(())
}

/// 切替失敗を呼び出し元へ返す。復帰にも失敗した場合は古い複製での操作を止める。
async fn fail_switch(
    connection: &BrowserConnection,
    current: &ApiWorkspaceId,
    attempted: Option<&ApiWorkspaceId>,
    error: String,
) -> String {
    match restore_connection(connection, current, attempted).await {
        Ok(()) => error,
        Err(recovery) => {
            BACKEND.with(|backend| *backend.borrow_mut() = None);
            format!(
                "{error}（元のWorkspaceへ戻れませんでした: {recovery}。ページを再読み込みしてください）"
            )
        }
    }
}

/// Root の再構築が成功したので、切替前の複製を保持へ移す。
pub fn finish_workspace_switch() {
    let previous = PREVIOUS.with(|previous| previous.borrow_mut().take());
    if let Some(previous) = previous {
        cache_workspace(CachedWorkspace {
            replica: previous.replica,
        });
    }
}

/// Root の再構築に失敗したとき、attachment と複製を元の Workspace へ戻す。
pub async fn rollback_workspace_switch() -> Result<(), String> {
    let (connection, current, commands) = BACKEND.with(|backend| {
        let backend = backend.borrow();
        let state = backend.as_ref().ok_or("Hostに接続していません")?;
        Ok::<_, String>((
            state.connection.clone(),
            state.workspace.clone(),
            state.commands.clone(),
        ))
    })?;
    let previous = PREVIOUS
        .with(|previous| {
            previous
                .borrow()
                .as_ref()
                .map(|previous| previous.workspace.clone())
        })
        .ok_or("切替前のWorkspaceがありません")?;
    if SWITCHING.with(|switching| switching.replace(true)) {
        return Err("Workspaceを切り替えています".to_owned());
    }
    let (done, drained) = oneshot::channel();
    if commands.unbounded_send(Command::Barrier { done }).is_err() {
        SWITCHING.with(|switching| switching.set(false));
        return Err("Workspace切替の待機を開始できません".to_owned());
    }
    if drained.await.is_err() {
        SWITCHING.with(|switching| switching.set(false));
        return Err("Workspace切替の待機に失敗しました".to_owned());
    }
    DRAINING.with(|draining| draining.set(true));
    let result = rollback_workspace_inner(&connection, &api_workspace(&current), &previous).await;
    DRAINING.with(|draining| draining.set(false));
    SWITCHING.with(|switching| switching.set(false));
    result
}

async fn rollback_workspace_inner(
    connection: &BrowserConnection,
    current: &ApiWorkspaceId,
    previous: &WorkspaceId,
) -> Result<(), String> {
    let previous_api = api_workspace(previous);
    // 切替先に接続中なので、先に閉じてから元の Workspace へ戻す。
    restore_connection(connection, &previous_api, Some(current)).await?;
    let previous = PREVIOUS
        .with(|previous| previous.borrow_mut().take())
        .ok_or("切替前のWorkspace状態がありません")?;
    let PreviousWorkspace {
        workspace: previous_workspace,
        workspace_identity: previous_identity,
        replica: previous_replica,
        target_was_cached,
    } = previous;
    let target = BACKEND.with(|backend| {
        let mut backend = backend.borrow_mut();
        let state = backend.as_mut().ok_or("Hostに接続していません")?;
        // 保持から復元した切替先は、rollback 後も次の復帰で使えるよう保持へ戻す。
        let target = if target_was_cached {
            Some(std::mem::replace(
                &mut *state.replica.borrow_mut(),
                previous_replica,
            ))
        } else {
            *state.replica.borrow_mut() = previous_replica;
            None
        };
        state.workspace = previous_workspace;
        state.workspace_identity = previous_identity;
        Ok::<_, String>(target)
    })?;
    if let Some(replica) = target {
        cache_workspace(CachedWorkspace { replica });
    }
    Ok(())
}

pub(super) fn api_workspace(id: &WorkspaceId) -> ApiWorkspaceId {
    ApiWorkspaceId::try_from(id).expect("workspace id is valid")
}

/// 接続中の Host/Workspace 識別子と切り替え候補を返す。
pub fn connection_info() -> Option<ConnectionInfo> {
    BACKEND.with(|backend| {
        let backend = backend.borrow();
        let state = backend.as_ref()?;
        let workspace_id = state.workspace.as_str().to_owned();
        let connected_name = crate::connected_workspace_name(&state.workspace_identity);
        let workspace_name = if connected_name.trim().is_empty() {
            workspace_id.clone()
        } else {
            connected_name.to_owned()
        };
        Some(ConnectionInfo {
            host_name: state.host_name.clone(),
            host_id: state.host_id.clone(),
            workspace_name,
            workspace_id,
            can_switch: state.can_switch,
            workspaces: state
                .workspaces
                .iter()
                .map(|workspace| WorkspaceChoice {
                    id: workspace.workspace_id.as_str().to_owned(),
                    name: workspace.name.clone(),
                })
                .collect(),
        })
    })
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

/// Editor が Host の文書変更通知を購読する。
///
/// Root の再構築でも同じ接続から購読し直せるよう、購読を都度追加する。
pub fn document_changes() -> Option<mpsc::UnboundedReceiver<()>> {
    BACKEND.with(|backend| {
        let backend = backend.borrow();
        let state = backend.as_ref()?;
        let (sender, receiver) = mpsc::unbounded();
        state.document_changes.borrow_mut().push(sender);
        Some(receiver)
    })
}

/// 別 Client が共有したタブ構成の変更を UI が購読する。
pub fn session_view_changes() -> Option<mpsc::UnboundedReceiver<()>> {
    BACKEND.with(|backend| {
        let backend = backend.borrow();
        let state = backend.as_ref()?;
        let (sender, receiver) = mpsc::unbounded();
        state.session_view_changes.borrow_mut().push(sender);
        Some(receiver)
    })
}

/// 共有 view state の watch。UI はこれを通じて適用対象を読む。
pub fn session_view_watch() -> SessionViewWatch {
    SESSION_VIEW.with(|session| session.borrow().watch().clone())
}

/// 共有中の workspace に対する snapshot を取り込み直す（再接続・復帰時）。
pub fn resync_session_view() {
    BACKEND.with(|backend| {
        if let Some(state) = backend.borrow().as_ref()
            && let Err(error) = state.commands.unbounded_send(Command::ResyncView)
        {
            web_sys::console::warn_1(&format!("Workspace view を再同期できません: {error}").into());
        }
    });
}

/// 自分のタブ構成を共有へ送る。null 相当の view は送らない。
pub fn publish_session_view(view: workbench::WorkspaceView) {
    BACKEND.with(|backend| {
        if let Some(state) = backend.borrow().as_ref()
            && let Err(error) = state.commands.unbounded_send(Command::PublishView { view })
        {
            web_sys::console::warn_1(&format!("Workspace view を共有できません: {error}").into());
        }
    });
}

/// Workspace を閉じたときに購読を解除する。
pub fn close_session_view() {
    BACKEND.with(|backend| {
        if let Some(state) = backend.borrow().as_ref() {
            let _ = state.commands.unbounded_send(Command::CloseView);
        }
    });
}

/// UIへ渡す同期Client。状態はthread_localにあるため、この型自体は空。
#[derive(Clone, Copy)]
pub struct WebBackend;

/// 切替中に未保存の変更を失わせないため、変更系の要求を呼び出し元へ返す。
fn ensure_mutation_allowed() -> Result<(), DocumentError> {
    web_switch::ensure_allowed(SWITCHING.with(Cell::get), Operation::Mutation)
        .map_err(DocumentError::io)
}

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
            ensure_mutation_allowed()?;
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
            Ok(cache)
        })
        .map_err(DocumentError::io)?
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
            ensure_mutation_allowed()?;
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
            ensure_mutation_allowed()?;
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
            ensure_mutation_allowed()?;
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
            ensure_mutation_allowed()?;
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
            ensure_mutation_allowed()?;
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
            ensure_mutation_allowed()?;
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
            ensure_mutation_allowed()?;
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
            ensure_mutation_allowed()?;
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
    fn identity(&self, _: &Path) -> Result<Option<WorkspaceIdentity>, WorkspaceError> {
        self.with(|state| Some(state.workspace_identity.clone()))
            .map_err(WorkspaceError::new)
    }

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
    fn load(&self) -> Result<Option<WorkspaceViewState>, WorkspaceError> {
        Ok(None)
    }
    fn save(&self, _: &WorkspaceViewState) -> Result<(), WorkspaceError> {
        Ok(())
    }
}

impl SettingsRepository for WebBackend {
    fn load(&self) -> Result<SettingsDocument, SettingsError> {
        // Web はこのブラウザーセッションだけの設定を持つ。保存先はまだ無い。
        Ok(SettingsDocument::empty(SettingsScope::User))
    }
    fn save(&self, _: &SettingsDocument) -> Result<(), SettingsError> {
        Ok(())
    }
}

fn workspace_from_shared(
    shared: &workbench::SharedWorkspaceView,
    expected: &WorkspaceId,
    root: PathBuf,
) -> Result<crate::WorkspaceViewState, WorkspaceError> {
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
    Ok(crate::WorkspaceViewState {
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
    snapshot: &crate::WorkspaceViewState,
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
pub fn restore_snapshot(root: PathBuf) -> Option<crate::WorkspaceViewState> {
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
            web_switch::ensure_allowed(SWITCHING.with(Cell::get), Operation::Mutation)
                .map_err(WorkspaceError::new)?;
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
            web_switch::ensure_allowed(SWITCHING.with(Cell::get), Operation::Mutation)
                .map_err(WorkspaceError::new)?;
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
        self.start_profile(cwd, columns, rows, None)
    }

    fn start_profile(
        &self,
        cwd: &Path,
        columns: u16,
        rows: u16,
        profile: Option<&terminal::TerminalProfileId>,
    ) -> Result<TerminalId, TerminalError> {
        let profile = profile.cloned();
        self.with(|state| {
            let mut replica = state.replica.borrow_mut();
            let local = TerminalId::new(format!("web-terminal-{}", replica.next_terminal));
            replica.next_terminal += 1;
            replica.terminals.insert(
                local.as_str().to_owned(),
                TerminalState {
                    host: None,
                    snapshot: None,
                    owner: None,
                    events: Vec::new(),
                    finished: false,
                },
            );
            let _ = state.commands.unbounded_send(Command::StartTerminal {
                local: local.clone(),
                cwd: Some(cwd.to_owned()),
                columns,
                rows,
                profile,
            });
            local
        })
        .map_err(TerminalError::new)
    }

    fn profiles(&self, _cwd: &Path) -> Result<terminal::TerminalProfiles, TerminalError> {
        self.with(|state| state.replica.borrow().profiles.clone())
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
        // Root の再構築でも同じ接続から購読し直せるよう、購読を都度追加する。
        self.with(|state| {
            let (sender, receiver) = mpsc::unbounded();
            state.changes.borrow_mut().push(sender);
            receiver
        })
        .ok()
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
                let snapshot = state
                    .replica
                    .borrow()
                    .terminals
                    .get(id.as_str())
                    .and_then(|state| state.snapshot.clone())
                    .ok_or_else(|| TerminalError::new("Terminalがありません"))?;
                // キャッシュだけを返さず、Host の現在の snapshot を取得し直す。
                // delta 欠番や開始応答前の取りこぼしはこの snapshot で回復する。
                let _ = state.commands.unbounded_send(Command::ResyncTerminals);
                Ok::<_, TerminalError>(snapshot)
            })
            .map_err(TerminalError::new)??;
        Ok(snapshot)
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
