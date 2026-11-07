use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex},
};

use ::document::Document;
use ::terminal::TerminalBackend;
use client_api::git as git_api;
use client_api::{
    DocumentCloseResponse, DocumentCreateResponse, DocumentEditResponse, DocumentEncoding,
    DocumentHistoryResponse, DocumentId, DocumentOpenResponse, DocumentSaveResponse, ErrorCode,
    EventBody, EventEnvelope, EventSequence, FORBIDDEN, FileTreeEntry, FileTreeKind,
    FileTreeResponse, INTERNAL, INVALID_PATH, INVALID_REQUEST, NOT_FOUND, ProtocolError,
    RequestBody, RequestEnvelope, ResponseBody, ResponseEnvelope, Revision, RevisionConflict,
    SessionId, SnapshotResponse, TerminalCommandResponse, TerminalId, TerminalInputRequest,
    TerminalResizeRequest, TerminalStartRequest, TerminalStartResponse, TerminalTerminateRequest,
    UNSUPPORTED, WorkspaceCloseResponse, WorkspaceConnectResponse, WorkspaceId,
    WorkspaceListResponse, WorkspaceRelativePath, WorkspaceSnapshot, WorkspaceSummary,
};
use document_state::{DocumentRegistry, DocumentResource, to_document_encoding};
use text::TextEdit;

use crate::{BackendStateError, WorkspaceEntryKind, WorkspaceFileBackend, WorkspacePathResolver};
use terminal_state::TerminalRegistry;

mod document;
mod git;
mod terminal;
mod workspace;

const MAX_RETAINED_EVENTS: usize = 1024;

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
    pub workbench_persistence: Option<Arc<dyn crate::WorkbenchPersistence>>,
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
            workbench_persistence: None,
            workbench_initial: workbench::VersionedViewState::default(),
        }
    }

    pub fn with_git(mut self, backend: Arc<dyn git_api::GitBackend>) -> Self {
        self.git = Some(backend);
        self
    }
    pub fn with_tasks(mut self, backend: Arc<dyn task::TaskBackend>) -> Self {
        self.tasks = Some(backend);
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
    pub(crate) fn execute(self) -> Result<client_api::WorkspaceOperationResult, BackendStateError> {
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
        request_id: client_api::RequestId,
        result: Result<client_api::WorkspaceOperationResult, BackendStateError>,
    ) -> DispatchOutcome {
        let result = result.and_then(|result| {
            let mut events = Vec::new();
            self.apply_git_updates(&workspace_id, &result, worktrees_changed, &mut events)?;
            if changed {
                events.push(
                    self.workspaces
                        .get_mut(&workspace_id)
                        .ok_or(BackendStateError::WorkspaceNotFound)?
                        .publish(EventBody::WorkspaceChanged { workspace_id })?,
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

    pub(crate) fn disconnect(&mut self, session_id: &SessionId) {
        self.connected_sessions.remove(session_id);
        for workspace in self.workspaces.values_mut() {
            for document in workspace.documents.values_mut() {
                document.attached_sessions.remove(session_id);
            }
            workspace.terminals.detach(session_id);
        }
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
            .map(|document| document.snapshot(&workspace.summary.workspace_id))
            .collect::<Vec<_>>();
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
                    git_repositories: workspace.git_statuses.values().cloned().collect(),
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
}

struct WorkspaceState {
    workbench: workbench::VersionedViewState,
    summary: WorkspaceSummary,
    resolver: WorkspacePathResolver,
    files: Arc<dyn WorkspaceFileBackend>,
    documents: DocumentRegistry,
    terminals: TerminalRegistry,
    operations: Arc<Mutex<crate::operations::WorkspaceOperations>>,
    events: EventJournal,
    git_statuses: HashMap<String, git_api::GitRepositoryStatus>,
    removed_worktrees: HashSet<String>,
    git_repositories: Option<HashSet<String>>,
    git_driver_started: bool,
}

impl WorkspaceState {
    fn new(registration: WorkspaceRegistration) -> Result<Self, BackendStateError> {
        if !registration.workbench_initial.is_consistent() {
            return Err(BackendStateError::InvalidWorkbench);
        }
        if let Some(view) = &registration.workbench_initial.view {
            workbench_state::validate_workspace(view, &registration.workspace_id)?;
        }
        Ok(Self {
            workbench: registration.workbench_initial,
            summary: WorkspaceSummary {
                workspace_id: registration.workspace_id.clone(),
                name: registration.name,
            },
            resolver: WorkspacePathResolver::new(registration.root)?,
            files: registration.files,
            documents: DocumentRegistry::new(),
            terminals: TerminalRegistry::new(registration.terminal),
            operations: Arc::new(Mutex::new(crate::operations::WorkspaceOperations::new(
                registration.git,
                registration.tasks,
                &registration.workspace_id,
            ))),
            events: EventJournal::default(),
            git_statuses: HashMap::new(),
            removed_worktrees: HashSet::new(),
            git_repositories: None,
            git_driver_started: false,
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

#[derive(Default)]
struct EventJournal {
    next_sequence: EventSequence,
    events: VecDeque<EventEnvelope>,
}

impl EventJournal {
    fn push(&mut self, body: EventBody) -> Result<EventEnvelope, BackendStateError> {
        let sequence = self
            .next_sequence
            .checked_next()
            .ok_or(BackendStateError::CounterOverflow)?;
        self.next_sequence = sequence;
        let event = EventEnvelope {
            event_sequence: sequence,
            body,
        };
        self.events.push_back(event.clone());
        if self.events.len() > MAX_RETAINED_EVENTS {
            self.events.pop_front();
        }
        Ok(event)
    }

    fn watermark(&self) -> EventSequence {
        self.next_sequence
    }
}

fn require_revision(
    document_id: &DocumentId,
    expected: Revision,
    actual: Revision,
) -> Result<(), BackendStateError> {
    if expected == actual {
        Ok(())
    } else {
        Err(BackendStateError::RevisionConflict(RevisionConflict {
            document_id: document_id.clone(),
            expected,
            actual,
        }))
    }
}

pub(crate) fn protocol_error(error: BackendStateError) -> ProtocolError {
    if let BackendStateError::Operation(message) = &error {
        let mut error = ProtocolError::new(ErrorCode::try_new(INTERNAL).expect("valid code"));
        error.detail = Some(message.clone());
        return error;
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
        BackendStateError::WorkspaceNotConnected
        | BackendStateError::DuplicateWorkspace
        | BackendStateError::Document(_)
        | BackendStateError::TerminalNotRunning
        | BackendStateError::InvalidWorkbench
        | BackendStateError::InvalidTerminalSize => INVALID_REQUEST,
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
