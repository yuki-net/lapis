use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex},
};

use ::document::Document;
use ::terminal::TerminalBackend;
use client_api::git as git_api;
use client_api::{
    DocumentCloseResponse, DocumentCreateResponse, DocumentEditResponse, DocumentEncoding,
    DocumentHistoryResponse, DocumentId, DocumentOpenResponse, DocumentSaveResponse, ErrorCode,
    EventBody, EventEnvelope, FORBIDDEN, FileTreeEntry, FileTreeKind, FileTreeResponse, INTERNAL,
    INVALID_PATH, INVALID_REQUEST, NOT_FOUND, ProtocolError, RequestBody, RequestEnvelope,
    ResponseBody, ResponseEnvelope, Revision, SessionId, SnapshotResponse, TerminalClaimRequest,
    TerminalCommandResponse, TerminalId, TerminalInputRequest, TerminalListProfilesRequest,
    TerminalListProfilesResponse, TerminalReleaseRequest, TerminalResizeRequest,
    TerminalStartRequest, TerminalStartResponse, TerminalTerminateRequest, UNSUPPORTED,
    WorkspaceCloseResponse, WorkspaceConnectResponse, WorkspaceId, WorkspaceIdentitySnapshot,
    WorkspaceListResponse, WorkspaceRelativePath, WorkspaceSnapshot, WorkspaceSummary,
};
use document_state::{DocumentRegistry, DocumentResource};
use text::TextEdit;

use crate::{BackendStateError, WorkspaceEntryKind, WorkspaceFileBackend, WorkspacePathResolver};
use terminal_state::TerminalRegistry;

mod document;
mod event;
mod git_state;
mod terminal;
mod view;
mod workspace;

pub(crate) use event::{EventJournal, require_revision};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendSession {
    pub session_id: SessionId,
    pub workspace_id: WorkspaceId,
    list_user_workspaces: bool,
}

impl BackendSession {
    pub fn new(session_id: SessionId, workspace_id: WorkspaceId) -> Self {
        Self {
            session_id,
            workspace_id,
            list_user_workspaces: false,
        }
    }

    /// 認証境界が明示的に許可した場合だけWorkspace一覧を広げる。
    pub fn with_user_workspace_listing(mut self, allowed: bool) -> Self {
        self.list_user_workspaces = allowed;
        self
    }
}

pub struct WorkspaceRegistration {
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub root: PathBuf,
    pub files: Arc<dyn WorkspaceFileBackend>,
    pub terminal: Option<Arc<dyn TerminalBackend>>,
    pub git: Option<Arc<dyn git_api::GitBackend>>,
    pub tasks: Option<Arc<dyn task::TaskBackend>>,
    /// Host が spawn したプロセスのレジストリ。Terminal / Task Run の所有関係を登録する。
    pub processes: Option<Arc<dyn process::ProcessRegistry>>,
    pub workbench_persistence: Option<Arc<dyn crate::WorkbenchPersistence>>,
    /// Document の編集権を無操作で解放するまでの時間。`None` は既定値。
    pub document_owner_idle_timeout: Option<std::time::Duration>,
    pub(crate) workbench_initial: workbench::VersionedViewState,
}

impl WorkspaceRegistration {
    pub fn new(
        workspace_id: WorkspaceId,
        name: impl Into<String>,
        root: PathBuf,
        files: Arc<dyn WorkspaceFileBackend>,
    ) -> Self {
        Self {
            workspace_id,
            name: name.into(),
            root,
            files,
            terminal: None,
            git: None,
            tasks: None,
            processes: None,
            workbench_persistence: None,
            document_owner_idle_timeout: None,
            workbench_initial: workbench::VersionedViewState::default(),
        }
    }

    /// Document の編集権を無操作で解放するまでの時間を変更する（テスト・設定用）。
    pub fn with_document_owner_idle_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.document_owner_idle_timeout = Some(timeout);
        self
    }

    pub fn with_git(mut self, backend: Arc<dyn git_api::GitBackend>) -> Self {
        self.git = Some(backend);
        self
    }
    pub fn with_tasks(mut self, backend: Arc<dyn task::TaskBackend>) -> Self {
        self.tasks = Some(backend);
        self
    }
    pub fn with_processes(mut self, processes: Arc<dyn process::ProcessRegistry>) -> Self {
        self.processes = Some(processes);
        self
    }
    pub fn with_terminal(mut self, terminal: Arc<dyn TerminalBackend>) -> Self {
        self.terminal = Some(terminal);
        self
    }
    pub fn with_workbench_persistence(
        mut self,
        persistence: Arc<dyn crate::WorkbenchPersistence>,
    ) -> Self {
        self.workbench_persistence = Some(persistence);
        self
    }
}

pub struct BackendState {
    removing_worktrees: HashSet<PathBuf>,
    active_operations: HashMap<WorkspaceId, usize>,
    workspaces: HashMap<WorkspaceId, WorkspaceState>,
    connected_sessions: HashMap<SessionId, WorkspaceId>,
    next_document_id: u64,
    document_epoch: u128,
    next_terminal_id: u64,
}

pub(crate) struct DispatchOutcome {
    pub(crate) response: ResponseEnvelope,
    pub(crate) events: Vec<PublishedEvent>,
}

pub(crate) struct PublishedEvent {
    pub(crate) workspace_id: WorkspaceId,
    pub(crate) event: EventEnvelope,
}

pub(crate) struct PreparedWorkspaceOperation {
    operations: Arc<Mutex<crate::operations::WorkspaceOperations>>,
    root: PathBuf,
    operation: client_api::WorkspaceOperation,
}

pub(crate) struct PreparedWorktreeLookup {
    operations: Arc<Mutex<crate::operations::WorkspaceOperations>>,
    root: PathBuf,
    worktree: editor_core::TaskId,
}

impl PreparedWorktreeLookup {
    pub(crate) fn execute(self) -> Result<PathBuf, BackendStateError> {
        let tree = self
            .operations
            .lock()
            .map_err(|_| BackendStateError::Operation("Workspace operation unavailable".into()))?
            .worktree(&self.root, &self.worktree)?;
        if tree.state == git_api::WorktreeState::Discarded {
            return Err(BackendStateError::WorkspaceDenied);
        }
        Ok(tree.path)
    }
}

impl PreparedWorkspaceOperation {
    pub(crate) fn execute(
        self,
    ) -> Result<client_api::WorkspaceOperationResponse, BackendStateError> {
        self.operations
            .lock()
            .map_err(|_| BackendStateError::Operation("Workspace operation unavailable".into()))?
            .execute(self.operation, &self.root)
    }
}

impl BackendState {
    pub(crate) fn commit_saved_workbench(
        &mut self,
        workspace_id: &WorkspaceId,
        expected: u64,
        saved: workbench::VersionedViewState,
    ) -> Result<PublishedEvent, BackendStateError> {
        let workspace = self
            .workspaces
            .get_mut(workspace_id)
            .ok_or(BackendStateError::WorkspaceNotFound)?;
        if workspace.workbench.revision != expected
            || saved.revision
                != expected
                    .checked_add(1)
                    .ok_or(BackendStateError::CounterOverflow)?
        {
            return Err(BackendStateError::Operation(
                "workbench persistence revision mismatch".into(),
            ));
        }
        workspace
            .events
            .watermark()
            .checked_next()
            .ok_or(BackendStateError::CounterOverflow)?;
        workspace.workbench = saved;
        workspace.publish(EventBody::WorkbenchChanged {
            workspace_id: workspace_id.clone(),
            revision: expected + 1,
        })
    }
    pub(crate) fn reserve_worktree_removal(
        &mut self,
        root: PathBuf,
    ) -> Result<(), BackendStateError> {
        if self
            .removing_worktrees
            .iter()
            .any(|other| root.starts_with(other) || other.starts_with(&root))
        {
            return Err(BackendStateError::Operation("Worktreeを削除中です".into()));
        }
        for (id, workspace) in &self.workspaces {
            let within = workspace.resolver.root().starts_with(&root);
            if workspace.terminals.active_in(&root)
                || workspace.documents.values().any(|document| {
                    (within
                        || document
                            .document
                            .path()
                            .is_some_and(|path| path.starts_with(&root)))
                        && (document.document.is_dirty() || !document.attached_sessions.is_empty())
                })
                || within
                    && (self
                        .connected_sessions
                        .values()
                        .any(|connected| connected == id)
                        || self.active_operations.get(id).copied().unwrap_or(0) != 0)
            {
                return Err(BackendStateError::Operation("Worktreeが使用中です。文書の未保存変更を保存・退避し、Terminalを終了して作業環境を切り替えてください".into()));
            }
        }
        self.removing_worktrees.insert(root);
        Ok(())
    }

    pub(crate) fn release_worktree_removal(&mut self, root: &std::path::Path) {
        self.removing_worktrees.remove(root);
    }

    fn require_available_workspace(&self, id: &WorkspaceId) -> Result<(), BackendStateError> {
        if self.workspaces.get(id).is_some_and(|workspace| {
            self.removing_worktrees
                .iter()
                .any(|root| workspace.resolver.root().starts_with(root))
        }) {
            return Err(BackendStateError::Operation("Worktreeを削除中です".into()));
        }
        Ok(())
    }
    pub(crate) fn prepare_worktree_lookup(
        &self,
        workspace_id: &WorkspaceId,
        worktree: editor_core::TaskId,
    ) -> Result<PreparedWorktreeLookup, BackendStateError> {
        let workspace = self
            .workspaces
            .get(workspace_id)
            .ok_or(BackendStateError::WorkspaceNotFound)?;
        Ok(PreparedWorktreeLookup {
            operations: workspace.operations.clone(),
            root: workspace.resolver.root().to_owned(),
            worktree,
        })
    }
    pub(crate) fn prepare_workspace_operation(
        &mut self,
        session: &BackendSession,
        request: client_api::WorkspaceOperationRequest,
    ) -> Result<PreparedWorkspaceOperation, BackendStateError> {
        self.require_workspace(session, &request.workspace_id)?;
        self.require_connected(session)?;
        self.require_available_workspace(&session.workspace_id)?;
        let workspace = self
            .workspaces
            .get(&session.workspace_id)
            .ok_or(BackendStateError::WorkspaceNotFound)?;
        *self
            .active_operations
            .entry(session.workspace_id.clone())
            .or_default() += 1;
        Ok(PreparedWorkspaceOperation {
            operations: workspace.operations.clone(),
            root: workspace.resolver.root().to_owned(),
            operation: request.operation,
        })
    }

    pub(crate) fn complete_workspace_operation(
        &mut self,
        workspace_id: WorkspaceId,
        changed: bool,
        worktrees_changed: bool,
        tasks_changed: bool,
        request_id: client_api::RequestId,
        result: Result<client_api::WorkspaceOperationResponse, BackendStateError>,
    ) -> DispatchOutcome {
        let result = result.and_then(|result| {
            let mut events = Vec::new();
            self.apply_git_updates(&workspace_id, &result, worktrees_changed, &mut events)?;
            if changed {
                events.push(
                    self.workspaces
                        .get_mut(&workspace_id)
                        .ok_or(BackendStateError::WorkspaceNotFound)?
                        .publish(EventBody::WorkspaceChanged {
                            workspace_id: workspace_id.clone(),
                        })?,
                );
            }
            if tasks_changed {
                events.push(
                    self.workspaces
                        .get_mut(&workspace_id)
                        .ok_or(BackendStateError::WorkspaceNotFound)?
                        .publish(EventBody::TasksChanged { workspace_id })?,
                );
            }
            Ok((ResponseBody::WorkspaceExecute(result), events))
        });
        match result {
            Ok((body, events)) => DispatchOutcome {
                response: ResponseEnvelope { request_id, body },
                events,
            },
            Err(error) => DispatchOutcome {
                response: ResponseEnvelope {
                    request_id,
                    body: ResponseBody::Error(protocol_error(error)),
                },
                events: Vec::new(),
            },
        }
    }

    pub(crate) fn finish_workspace_operation(&mut self, workspace: &WorkspaceId) {
        if let Some(count) = self.active_operations.get_mut(workspace) {
            *count = count.saturating_sub(1);
        }
    }

    pub(crate) fn relocate_document(
        &mut self,
        source: WorkspaceId,
        destination: WorkspaceId,
        document_id: DocumentId,
        revision: Revision,
        path: WorkspaceRelativePath,
    ) -> Result<Vec<PublishedEvent>, BackendStateError> {
        self.require_available_workspace(&source)?;
        self.require_available_workspace(&destination)?;
        if source == destination {
            return Err(BackendStateError::WorkspaceDenied);
        }
        let source_workspace = self
            .workspaces
            .get(&source)
            .ok_or(BackendStateError::WorkspaceNotFound)?;
        let resource = source_workspace
            .documents
            .get(&document_id)
            .ok_or(BackendStateError::DocumentNotFound)?;
        require_revision(&document_id, revision, resource.revision())?;
        let target = self
            .workspaces
            .get(&destination)
            .ok_or(BackendStateError::WorkspaceNotFound)?;
        let absolute = self.document_target(&destination, &path)?;
        let path = canonical_document_path(&target.resolver, &absolute)?;
        if target.documents.contains_path(&absolute) {
            return Err(::document::DocumentError::conflict(
                "保存先は開いている文書と重複しています",
            )
            .into());
        }
        if target.files.fingerprint(&absolute)?.is_some() {
            return Err(::document::DocumentError::conflict("保存先は既に存在します").into());
        }
        let fingerprint =
            target
                .files
                .write_file(&absolute, &resource.document.encoded_bytes(), None)?;
        let source_workspace = self
            .workspaces
            .get_mut(&source)
            .ok_or(BackendStateError::WorkspaceNotFound)?;
        let mut resource = source_workspace
            .documents
            .remove(&document_id)
            .ok_or(BackendStateError::DocumentNotFound)?;
        if let Some(previous) = resource.document.path() {
            source_workspace.documents.remove_path(previous);
        }
        let closed = source_workspace.publish(EventBody::DocumentClosed {
            document_id: document_id.clone(),
        })?;
        resource.path = Some(path.clone());
        resource.document.mark_saved(absolute.clone(), fingerprint);
        resource.attached_sessions.clear();
        let target = self
            .workspaces
            .get_mut(&destination)
            .ok_or(BackendStateError::WorkspaceNotFound)?;
        let snapshot = resource.snapshot(&destination);
        target.documents.insert_path(absolute, document_id.clone());
        target.documents.insert(document_id, resource);
        let replaced = target.publish(EventBody::DocumentReplaced { document: snapshot })?;
        Ok(vec![closed, replaced])
    }
    pub fn register_workspace(
        &mut self,
        registration: WorkspaceRegistration,
    ) -> Result<(), BackendStateError> {
        if registration.workbench_persistence.is_some() {
            return Err(BackendStateError::Operation(
                "persistent workbench requires BackendService::register_workspace".into(),
            ));
        }
        let workspace_id = registration.workspace_id.clone();
        if self.workspaces.contains_key(&workspace_id) {
            return Err(BackendStateError::DuplicateWorkspace);
        }
        let workspace = WorkspaceState::new(registration)?;
        if self
            .removing_worktrees
            .iter()
            .any(|root| workspace.resolver.root().starts_with(root))
        {
            return Err(BackendStateError::Operation("Worktreeを削除中です".into()));
        }
        self.workspaces.insert(workspace_id, workspace);
        Ok(())
    }

    pub fn new(
        registrations: impl IntoIterator<Item = WorkspaceRegistration>,
    ) -> Result<Self, BackendStateError> {
        let mut workspaces = HashMap::new();
        for registration in registrations {
            if registration.workbench_persistence.is_some() {
                return Err(BackendStateError::Operation(
                    "persistent workbench requires BackendService::register_workspace".into(),
                ));
            }
            let workspace_id = registration.workspace_id.clone();
            let workspace = WorkspaceState::new(registration)?;
            if workspaces.insert(workspace_id, workspace).is_some() {
                return Err(BackendStateError::DuplicateWorkspace);
            }
        }
        Ok(Self {
            removing_worktrees: HashSet::new(),
            active_operations: HashMap::new(),
            workspaces,
            connected_sessions: HashMap::new(),
            next_document_id: 1,
            document_epoch: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            next_terminal_id: 1,
        })
    }

    pub(crate) fn dispatch(
        &mut self,
        session: &BackendSession,
        request: RequestEnvelope,
    ) -> DispatchOutcome {
        let request_id = request.request_id.clone();
        let result = self.dispatch_body(session, request.body);
        match result {
            Ok((body, events)) => DispatchOutcome {
                response: ResponseEnvelope { request_id, body },
                events,
            },
            Err(error) => DispatchOutcome {
                response: ResponseEnvelope {
                    request_id,
                    body: ResponseBody::Error(protocol_error(error)),
                },
                events: Vec::new(),
            },
        }
    }

    pub(crate) fn disconnect(&mut self, session_id: &SessionId) -> Vec<PublishedEvent> {
        self.connected_sessions.remove(session_id);
        let mut published = Vec::new();
        for workspace in self.workspaces.values_mut() {
            workspace.view.unsubscribe(session_id);
            for document in workspace.documents.values_mut() {
                document.attached_sessions.remove(session_id);
            }
            for document_id in workspace.documents.release_owned_by(session_id) {
                if let Ok(event) = workspace.publish(EventBody::DocumentOwner {
                    document_id,
                    owner: None,
                }) {
                    published.push(event);
                }
            }
            workspace.terminals.detach(session_id);
        }
        published
    }

    fn dispatch_body(
        &mut self,
        session: &BackendSession,
        request: RequestBody,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        self.require_available_workspace(&session.workspace_id)?;
        match request {
            RequestBody::WorkbenchGet(request) => {
                self.require_workspace(session, &request.workspace_id)?;
                self.require_connected(session)?;
                let workspace = self.workspace_mut(session)?;
                Ok((
                    ResponseBody::WorkbenchGet(workbench_state::snapshot(&workspace.workbench)?),
                    Vec::new(),
                ))
            }
            RequestBody::WorkbenchPut(request) => {
                self.require_workspace(session, &request.workspace_id)?;
                self.require_connected(session)?;
                let current = self
                    .workspace(session)?
                    .workbench
                    .view
                    .as_ref()
                    .and_then(|view| view.workspace.as_ref())
                    .cloned();
                let view = workbench_state::parse_update_view(request.view, current.as_ref())?;
                workbench_state::validate_workspace(&view, &request.workspace_id)?;
                let workspace = self.workspace_mut(session)?;
                if workspace.workbench.revision == request.expected_revision {
                    workspace
                        .events
                        .watermark()
                        .checked_next()
                        .ok_or(BackendStateError::CounterOverflow)?;
                }
                match workspace.workbench.update(request.expected_revision, view) {
                    Ok(revision) => {
                        let snapshot = workbench_state::snapshot(&workspace.workbench)?;
                        let event = workspace.publish(EventBody::WorkbenchChanged {
                            workspace_id: request.workspace_id,
                            revision,
                        })?;
                        Ok((
                            ResponseBody::WorkbenchPut(client_api::WorkbenchPutResponse::Updated(
                                snapshot,
                            )),
                            vec![event],
                        ))
                    }
                    Err(workbench::ViewUpdateError::Conflict { .. }) => {
                        let snapshot = workbench_state::snapshot(&workspace.workbench)?;
                        Ok((
                            ResponseBody::WorkbenchPut(client_api::WorkbenchPutResponse::Conflict(
                                snapshot,
                            )),
                            Vec::new(),
                        ))
                    }
                    Err(workbench::ViewUpdateError::RevisionOverflow) => {
                        Err(BackendStateError::CounterOverflow)
                    }
                }
            }
            RequestBody::CreateDirectory(request) => {
                self.require_workspace(session, &request.workspace_id)?;
                self.require_connected(session)?;
                let workspace = self.workspace_mut(session)?;
                let directory = workspace.resolver.resolve_new_file(&request.path)?;
                workspace.files.create_directory(&directory)?;
                let event = workspace.publish(EventBody::WorkspaceChanged {
                    workspace_id: request.workspace_id,
                })?;
                Ok((
                    ResponseBody::CreateDirectory(client_api::CreateDirectoryResponse {
                        path: request.path,
                    }),
                    vec![event],
                ))
            }
            RequestBody::WorkspaceExecute(_)
            | RequestBody::HostRegister(_)
            | RequestBody::HostWorktree(_)
            | RequestBody::DocumentRelocate(_) => Err(BackendStateError::Unsupported),
            RequestBody::DocumentUntitled(request) => {
                self.require_workspace(session, &request.workspace_id)?;
                self.require_connected(session)?;
                self.document_untitled(session)
            }
            RequestBody::DocumentInspect(request) => {
                self.require_connected(session)?;
                self.document_inspect(session, request.document_id)
            }
            RequestBody::DocumentSaveAs(request) => {
                self.require_connected(session)?;
                self.document_save_as(session, request)
            }
            RequestBody::DocumentReload(request) => {
                self.require_connected(session)?;
                self.document_reload(session, request.document_id, request.base_revision)
            }
            RequestBody::DocumentDiscard(request) => {
                self.require_connected(session)?;
                self.document_discard(session, request)
            }
            RequestBody::WorkspaceList(_) => self.workspace_list(session),
            RequestBody::WorkspaceConnect(request) => {
                self.workspace_connect(session, request.workspace_id)
            }
            RequestBody::WorkspaceClose(request) => {
                self.workspace_close(session, request.workspace_id)
            }
            RequestBody::WorkspaceViewSubscribe(request) => {
                self.require_workspace(session, &request.workspace_id)?;
                self.require_connected(session)?;
                let workspace = self.workspace_mut(session)?;
                let snapshot = workspace.view.subscribe(&session.session_id);
                Ok((ResponseBody::WorkspaceViewSubscribe(snapshot), Vec::new()))
            }
            RequestBody::WorkspaceViewUnsubscribe(request) => {
                self.require_workspace(session, &request.workspace_id)?;
                self.require_connected(session)?;
                let workspace = self.workspace_mut(session)?;
                workspace.view.unsubscribe(&session.session_id);
                Ok((
                    ResponseBody::WorkspaceViewUnsubscribe(
                        client_api::WorkspaceViewUnsubscribeResponse {
                            workspace_id: request.workspace_id,
                        },
                    ),
                    Vec::new(),
                ))
            }
            RequestBody::WorkspaceViewUpdate(request) => {
                self.require_workspace(session, &request.workspace_id)?;
                self.require_connected(session)?;
                workbench_state::validate_session_view(&request.view)?;
                let workspace = self.workspace_mut(session)?;
                let update = workspace.view.update(request.view)?;
                let response =
                    ResponseBody::WorkspaceViewUpdate(client_api::WorkspaceViewUpdateResponse {
                        revision: update.revision,
                    });
                let events = match update.broadcast {
                    Some(view) => vec![workspace.publish(EventBody::WorkspaceViewChanged {
                        workspace_id: request.workspace_id,
                        revision: update.revision,
                        view,
                    })?],
                    None => Vec::new(),
                };
                Ok((response, events))
            }
            RequestBody::FileTree(request) => {
                self.require_connected(session)?;
                self.file_tree(session, request.path)
            }
            RequestBody::DocumentOpen(request) => {
                self.require_workspace(session, &request.workspace_id)?;
                self.require_connected(session)?;
                self.document_open(session, request.path)
            }
            RequestBody::DocumentCreate(request) => {
                self.require_workspace(session, &request.workspace_id)?;
                self.require_connected(session)?;
                self.document_create(session, request.path, request.encoding, request.content)
            }
            RequestBody::DocumentEdit(request) => {
                self.require_connected(session)?;
                self.document_edit(session, request)
            }
            RequestBody::DocumentClaim(request) => {
                self.require_connected(session)?;
                self.document_claim(session, request)
            }
            RequestBody::DocumentRelease(request) => {
                self.require_connected(session)?;
                self.document_release(session, request)
            }
            RequestBody::DocumentSave(request) => {
                self.require_connected(session)?;
                self.document_save(session, request.document_id, request.base_revision)
            }
            RequestBody::DocumentUndo(request) => {
                self.require_connected(session)?;
                self.document_history(session, request.document_id, request.base_revision, true)
            }
            RequestBody::DocumentRedo(request) => {
                self.require_connected(session)?;
                self.document_history(session, request.document_id, request.base_revision, false)
            }
            RequestBody::DocumentClose(request) => {
                self.require_connected(session)?;
                self.document_close(session, request.document_id)
            }
            RequestBody::SnapshotResync(request) => {
                self.require_workspace(session, &request.workspace_id)?;
                self.require_connected(session)?;
                self.snapshot(session)
            }
            RequestBody::TerminalStart(request) => {
                self.require_workspace(session, &request.workspace_id)?;
                self.require_connected(session)?;
                self.terminal_start(session, request)
            }
            RequestBody::TerminalListProfiles(request) => {
                self.terminal_list_profiles(session, request)
            }
            RequestBody::TerminalInput(request) => {
                self.require_connected(session)?;
                self.terminal_input(session, request)
            }
            RequestBody::TerminalResize(request) => {
                self.require_connected(session)?;
                self.terminal_resize(session, request)
            }
            RequestBody::TerminalTerminate(request) => {
                self.require_connected(session)?;
                self.terminal_terminate(session, request)
            }
            RequestBody::TerminalClaim(request) => {
                self.require_connected(session)?;
                self.terminal_claim(session, request)
            }
            RequestBody::TerminalRelease(request) => {
                self.require_connected(session)?;
                self.terminal_release(session, request)
            }
        }
    }

    fn snapshot(
        &mut self,
        session: &BackendSession,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let workspace = self.workspace_mut(session)?;
        let documents = workspace
            .documents
            .values()
            .map(|document| document::document_snapshot(workspace, document))
            .collect::<Result<Vec<_>, _>>()?;
        let terminals = workspace
            .terminals
            .snapshots_for_session(&session.session_id, &workspace.summary.workspace_id);
        Ok((
            ResponseBody::SnapshotResync(SnapshotResponse {
                snapshot: WorkspaceSnapshot {
                    event_watermark: workspace.events.watermark(),
                    workspace: workspace.summary.clone(),
                    documents,
                    terminals,
                    git_repositories: workspace.git.statuses().cloned().collect(),
                },
            }),
            Vec::new(),
        ))
    }

    fn require_workspace(
        &self,
        session: &BackendSession,
        workspace_id: &WorkspaceId,
    ) -> Result<(), BackendStateError> {
        if &session.workspace_id == workspace_id {
            Ok(())
        } else {
            Err(BackendStateError::WorkspaceDenied)
        }
    }

    pub(crate) fn require_connected(
        &self,
        session: &BackendSession,
    ) -> Result<(), BackendStateError> {
        match self.connected_sessions.get(&session.session_id) {
            Some(workspace_id) if workspace_id == &session.workspace_id => Ok(()),
            _ => Err(BackendStateError::WorkspaceNotConnected),
        }
    }

    pub(crate) fn has_connected_session(&self, session_id: &SessionId) -> bool {
        self.connected_sessions.contains_key(session_id)
    }

    fn workspace(&self, session: &BackendSession) -> Result<&WorkspaceState, BackendStateError> {
        self.workspaces
            .get(&session.workspace_id)
            .ok_or(BackendStateError::WorkspaceNotFound)
    }

    fn workspace_mut(
        &mut self,
        session: &BackendSession,
    ) -> Result<&mut WorkspaceState, BackendStateError> {
        self.workspaces
            .get_mut(&session.workspace_id)
            .ok_or(BackendStateError::WorkspaceNotFound)
    }

    fn allocate_document_id(&mut self) -> Result<DocumentId, BackendStateError> {
        let value = self.next_document_id;
        self.next_document_id = self
            .next_document_id
            .checked_add(1)
            .ok_or(BackendStateError::CounterOverflow)?;
        DocumentId::try_new(format!(
            "document-{}-{}-{value}",
            std::process::id(),
            self.document_epoch
        ))
        .map_err(|_| BackendStateError::CounterOverflow)
    }

    fn allocate_terminal_id(&mut self) -> Result<TerminalId, BackendStateError> {
        let value = self.next_terminal_id;
        self.next_terminal_id = self
            .next_terminal_id
            .checked_add(1)
            .ok_or(BackendStateError::CounterOverflow)?;
        TerminalId::try_new(format!(
            "terminal-{}-{}-{value}",
            std::process::id(),
            self.document_epoch
        ))
        .map_err(|_| BackendStateError::CounterOverflow)
    }

    /// Terminal 出力の到着通知を受け取る。Workspace ごとに 1 度だけ取得する。
    pub(crate) fn take_terminal_changes(
        &mut self,
    ) -> Vec<futures::channel::mpsc::UnboundedReceiver<()>> {
        self.workspaces
            .values()
            .filter_map(|workspace| workspace.terminals.take_changes())
            .collect()
    }

    pub(crate) fn poll_terminals(&mut self) -> Result<Vec<PublishedEvent>, BackendStateError> {
        let mut published = Vec::new();
        for workspace in self.workspaces.values_mut() {
            let events = workspace.terminals.poll()?;
            for event in events {
                published.push(workspace.publish(event)?);
            }
        }
        Ok(published)
    }

    pub(crate) fn poll_document_owners(
        &mut self,
    ) -> Result<Vec<PublishedEvent>, BackendStateError> {
        let mut published = Vec::new();
        for workspace in self.workspaces.values_mut() {
            for document_id in workspace.documents.poll_owners() {
                published.push(workspace.publish(EventBody::DocumentOwner {
                    document_id,
                    owner: None,
                })?);
            }
        }
        Ok(published)
    }

    /// Adapter が監視したファイル・Task の変化を event へ変換する。
    /// 変化は1回の Runtime loop でまとめて1 event にする。
    pub(crate) fn poll_external_changes(
        &mut self,
    ) -> Result<Vec<PublishedEvent>, BackendStateError> {
        let mut published = Vec::new();
        for workspace in self.workspaces.values_mut() {
            let workspace_id = workspace.summary.workspace_id.clone();
            if take_changed(&mut workspace.file_changes) {
                published.push(workspace.publish(EventBody::FilesChanged {
                    workspace_id: workspace_id.clone(),
                })?);
            }
            if take_changed(&mut workspace.task_changes) {
                published.push(workspace.publish(EventBody::TasksChanged { workspace_id })?);
            }
        }
        Ok(published)
    }
}

/// Workspace root のディレクトリ名。root 自体が名前を持たない場合も空にしない。
fn root_name(root: &std::path::Path) -> String {
    root.file_name()
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| root.to_string_lossy().trim().to_owned())
}

/// 溜まった通知を1回にまとめる。監視が切れた場合も再同期を促す。
fn take_changed(slot: &mut Option<std::sync::mpsc::Receiver<()>>) -> bool {
    let Some(receiver) = slot.take() else {
        return false;
    };
    let mut changed = false;
    let mut disconnected = false;
    loop {
        match receiver.try_recv() {
            Ok(()) => changed = true,
            Err(std::sync::mpsc::TryRecvError::Empty) => break,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                changed = true;
                disconnected = true;
                break;
            }
        }
    }
    if !disconnected {
        *slot = Some(receiver);
    }
    changed
}

struct WorkspaceState {
    workbench: workbench::VersionedViewState,
    view: view::SessionViewState,
    summary: WorkspaceSummary,
    identity: WorkspaceIdentitySnapshot,
    resolver: WorkspacePathResolver,
    files: Arc<dyn WorkspaceFileBackend>,
    documents: DocumentRegistry,
    terminals: TerminalRegistry,
    operations: Arc<Mutex<crate::operations::WorkspaceOperations>>,
    events: EventJournal,
    git: git_state::GitState,
    file_changes: Option<std::sync::mpsc::Receiver<()>>,
    task_changes: Option<std::sync::mpsc::Receiver<()>>,
}

impl WorkspaceState {
    fn new(registration: WorkspaceRegistration) -> Result<Self, BackendStateError> {
        if !registration.workbench_initial.is_consistent() {
            return Err(BackendStateError::InvalidWorkbench);
        }
        if let Some(view) = &registration.workbench_initial.view {
            workbench_state::validate_workspace(view, &registration.workspace_id)?;
        }
        // 監視は状態の正本ではなく、変更のきっかけだけを Runtime へ渡す。
        let file_changes = registration.files.changes();
        let task_changes = registration
            .tasks
            .as_ref()
            .and_then(|tasks| tasks.changes());
        let documents = match registration.document_owner_idle_timeout {
            Some(timeout) => DocumentRegistry::new().with_owner_idle_timeout(timeout),
            None => DocumentRegistry::new(),
        };
        let identity = WorkspaceIdentitySnapshot {
            workspace_id: registration.workspace_id.clone(),
            name: registration.name.clone(),
            root_name: root_name(&registration.root),
        };
        let mut terminals = TerminalRegistry::new(registration.terminal);
        if let Some(processes) = registration.processes.clone() {
            terminals = terminals.with_processes(
                processes,
                editor_core::WorkspaceId::new(registration.workspace_id.as_str()),
            );
        }
        Ok(Self {
            workbench: registration.workbench_initial,
            view: view::SessionViewState::default(),
            summary: WorkspaceSummary {
                workspace_id: registration.workspace_id.clone(),
                name: registration.name,
            },
            identity,
            resolver: WorkspacePathResolver::new(registration.root)?,
            files: registration.files,
            documents,
            terminals,
            operations: Arc::new(Mutex::new(crate::operations::WorkspaceOperations::new(
                registration.git,
                registration.tasks,
                registration.processes,
                &registration.workspace_id,
            ))),
            events: EventJournal::default(),
            git: git_state::GitState::default(),
            file_changes,
            task_changes,
        })
    }

    fn require_attached_document_mut(
        &mut self,
        session: &BackendSession,
        document_id: &DocumentId,
    ) -> Result<&mut DocumentResource, BackendStateError> {
        Ok(self
            .documents
            .require_attached_mut(&session.session_id, document_id)?)
    }

    fn publish(&mut self, body: EventBody) -> Result<PublishedEvent, BackendStateError> {
        let event = self.events.push(body)?;
        Ok(PublishedEvent {
            workspace_id: self.summary.workspace_id.clone(),
            event,
        })
    }
}

pub(crate) fn protocol_error(error: BackendStateError) -> ProtocolError {
    if let BackendStateError::Operation(message) = &error {
        let mut error = ProtocolError::new(ErrorCode::try_new(INTERNAL).expect("valid code"));
        error.detail = Some(message.clone());
        return error;
    }
    if matches!(&error, BackendStateError::DocumentNotOwner) {
        return ProtocolError::new(ErrorCode::try_new(INVALID_REQUEST).expect("valid code"))
            .with_detail("他のClientが編集中です");
    }
    if matches!(&error, BackendStateError::TerminalNotOwner) {
        return ProtocolError::new(ErrorCode::try_new(INVALID_REQUEST).expect("valid code"))
            .with_detail("他のClientが操作中です");
    }
    if let BackendStateError::Document(ref document) = error
        && document.kind() == ::document::DocumentErrorKind::Conflict
    {
        return ProtocolError::new(ErrorCode::try_new("document_conflict").expect("valid code"));
    }
    if let BackendStateError::RevisionConflict(conflict) = error {
        return ProtocolError::revision_conflict(conflict);
    }
    let code = match error {
        BackendStateError::WorkspaceNotFound
        | BackendStateError::DocumentNotFound
        | BackendStateError::TerminalNotFound => NOT_FOUND,
        BackendStateError::WorkspaceDenied
        | BackendStateError::DocumentNotAttached
        | BackendStateError::TerminalNotAttached => FORBIDDEN,
        BackendStateError::DocumentNotOwner => INVALID_REQUEST,
        BackendStateError::WorkspaceNotConnected
        | BackendStateError::DuplicateWorkspace
        | BackendStateError::Document(_)
        | BackendStateError::TerminalNotRunning
        | BackendStateError::TerminalNotOwner
        | BackendStateError::InvalidWorkbench
        | BackendStateError::InvalidTerminalSize
        | BackendStateError::TerminalProfileDenied => INVALID_REQUEST,
        BackendStateError::Path(_) => INVALID_PATH,
        BackendStateError::CounterOverflow
        | BackendStateError::Terminal(_)
        | BackendStateError::Operation(_) => INTERNAL,
        BackendStateError::Unsupported => UNSUPPORTED,
        BackendStateError::RevisionConflict(_) => unreachable!("handled above"),
    };
    ProtocolError::new(ErrorCode::try_new(code).expect("built-in error code must be valid"))
}

fn canonical_document_path(
    resolver: &WorkspacePathResolver,
    absolute: &std::path::Path,
) -> Result<WorkspaceRelativePath, BackendStateError> {
    let relative = absolute
        .strip_prefix(resolver.root())
        .map_err(|_| crate::PathSecurityError::WorkspaceEscape)?;
    let relative = relative
        .to_str()
        .ok_or(crate::PathSecurityError::InvalidEntry)?;
    WorkspaceRelativePath::parse(relative.replace('\\', "/"))
        .map_err(|_| crate::PathSecurityError::InvalidEntry.into())
}

#[cfg(test)]
mod tests {
    use super::{root_name, take_changed};
    use std::sync::mpsc;

    #[test]
    fn root_name_uses_the_last_directory_and_never_becomes_empty() {
        assert_eq!(root_name(std::path::Path::new("repo")), "repo");
        assert_eq!(root_name(std::path::Path::new("nested/repo")), "repo");
        assert_eq!(root_name(std::path::Path::new("/")), "/");
    }

    #[test]
    fn take_changed_coalesces_notifications_and_keeps_the_receiver() {
        let (sender, receiver) = mpsc::sync_channel(2);
        let mut slot = Some(receiver);
        assert!(!take_changed(&mut slot));
        sender.send(()).unwrap();
        sender.send(()).unwrap();
        assert!(take_changed(&mut slot), "溜まった通知を 1 回にまとめる");
        assert!(slot.is_some(), "監視は継続する");
        assert!(!take_changed(&mut slot));
    }

    #[test]
    fn take_changed_resyncs_when_the_watcher_disconnects() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let mut slot = Some(receiver);
        drop(sender);
        assert!(take_changed(&mut slot), "切断時は再同期が必要");
        assert!(slot.is_none(), "切断した購読は破棄する");
    }
}
