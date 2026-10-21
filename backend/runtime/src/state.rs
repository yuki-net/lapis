use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex},
};

use lapis_client_api::{
    DocumentCloseResponse, DocumentCreateResponse, DocumentEditResponse, DocumentEncoding,
    DocumentHistoryResponse, DocumentId, DocumentOpenResponse, DocumentSaveResponse,
    DocumentSnapshot, ErrorCode, EventBody, EventEnvelope, EventSequence, FORBIDDEN, FileTreeEntry,
    FileTreeKind, FileTreeResponse, INTERNAL, INVALID_PATH, INVALID_REQUEST, NOT_FOUND,
    ProtocolError, RequestBody, RequestEnvelope, ResponseBody, ResponseEnvelope, Revision,
    RevisionConflict, SessionId, SnapshotResponse, TerminalCommandResponse, TerminalId,
    TerminalInputRequest, TerminalResizeRequest, TerminalStartRequest, TerminalStartResponse,
    TerminalTerminateRequest, UNSUPPORTED, WorkspaceCloseResponse, WorkspaceConnectResponse,
    WorkspaceId, WorkspaceListResponse, WorkspaceRelativePath, WorkspaceSnapshot, WorkspaceSummary,
};
use lapis_document::{Document, Encoding};
use lapis_terminal::TerminalBackend;
use lapis_text::TextEdit;

use crate::{
    BackendStateError, WorkspaceEntryKind, WorkspaceFileBackend, WorkspacePathResolver,
    terminal_state::TerminalRegistry,
};

const MAX_RETAINED_EVENTS: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendSession {
    pub session_id: SessionId,
    pub workspace_id: WorkspaceId,
}

impl BackendSession {
    pub fn new(session_id: SessionId, workspace_id: WorkspaceId) -> Self {
        Self {
            session_id,
            workspace_id,
        }
    }
}

pub struct WorkspaceRegistration {
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub root: PathBuf,
    pub files: Arc<dyn WorkspaceFileBackend>,
    pub terminal: Option<Arc<dyn TerminalBackend>>,
    pub git: Option<Arc<dyn lapis_git::GitBackend>>,
    pub tasks: Option<Arc<dyn lapis_task::TaskBackend>>,
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
        }
    }

    pub fn with_git(mut self, backend: Arc<dyn lapis_git::GitBackend>) -> Self {
        self.git = Some(backend);
        self
    }
    pub fn with_tasks(mut self, backend: Arc<dyn lapis_task::TaskBackend>) -> Self {
        self.tasks = Some(backend);
        self
    }
    pub fn with_terminal(mut self, terminal: Arc<dyn TerminalBackend>) -> Self {
        self.terminal = Some(terminal);
        self
    }
}

pub struct BackendState {
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
    operation: lapis_client_api::WorkspaceOperation,
}

impl PreparedWorkspaceOperation {
    pub(crate) fn execute(
        self,
    ) -> Result<lapis_client_api::WorkspaceOperationResult, BackendStateError> {
        self.operations
            .lock()
            .map_err(|_| BackendStateError::Operation("Workspace operation unavailable".into()))?
            .execute(self.operation, &self.root)
    }
}

impl BackendState {
    pub(crate) fn prepare_workspace_operation(
        &self,
        session: &BackendSession,
        request: lapis_client_api::WorkspaceOperationRequest,
    ) -> Result<PreparedWorkspaceOperation, BackendStateError> {
        self.require_workspace(session, &request.workspace_id)?;
        self.require_connected(session)?;
        let workspace = self
            .workspaces
            .get(&session.workspace_id)
            .ok_or(BackendStateError::WorkspaceNotFound)?;
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
        request_id: lapis_client_api::RequestId,
        result: Result<lapis_client_api::WorkspaceOperationResult, BackendStateError>,
    ) -> DispatchOutcome {
        let result = result.and_then(|result| {
            let events = if changed {
                vec![
                    self.workspaces
                        .get_mut(&workspace_id)
                        .ok_or(BackendStateError::WorkspaceNotFound)?
                        .publish(EventBody::WorkspaceChanged { workspace_id })?,
                ]
            } else {
                Vec::new()
            };
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

    pub(crate) fn relocate_document(
        &mut self,
        source: WorkspaceId,
        destination: WorkspaceId,
        document_id: DocumentId,
        revision: Revision,
        path: WorkspaceRelativePath,
    ) -> Result<Vec<PublishedEvent>, BackendStateError> {
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
        if target.documents_by_path.contains_key(&absolute) {
            return Err(lapis_document::DocumentError::conflict(
                "保存先は開いている文書と重複しています",
            )
            .into());
        }
        if target.files.fingerprint(&absolute)?.is_some() {
            return Err(lapis_document::DocumentError::conflict("保存先は既に存在します").into());
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
            source_workspace.documents_by_path.remove(previous);
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
        let snapshot = resource.snapshot(&destination)?;
        target
            .documents_by_path
            .insert(absolute, document_id.clone());
        target.documents.insert(document_id, resource);
        let replaced = target.publish(EventBody::DocumentReplaced { document: snapshot })?;
        Ok(vec![closed, replaced])
    }
    pub fn register_workspace(
        &mut self,
        registration: WorkspaceRegistration,
    ) -> Result<(), BackendStateError> {
        let workspace_id = registration.workspace_id.clone();
        if self.workspaces.contains_key(&workspace_id) {
            return Err(BackendStateError::DuplicateWorkspace);
        }
        let workspace = WorkspaceState::new(registration)?;
        self.workspaces.insert(workspace_id, workspace);
        Ok(())
    }

    pub fn new(
        registrations: impl IntoIterator<Item = WorkspaceRegistration>,
    ) -> Result<Self, BackendStateError> {
        let mut workspaces = HashMap::new();
        for registration in registrations {
            let workspace_id = registration.workspace_id.clone();
            let workspace = WorkspaceState::new(registration)?;
            if workspaces.insert(workspace_id, workspace).is_some() {
                return Err(BackendStateError::DuplicateWorkspace);
            }
        }
        Ok(Self {
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
        match request {
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
                    ResponseBody::CreateDirectory(lapis_client_api::CreateDirectoryResponse {
                        path: request.path,
                    }),
                    vec![event],
                ))
            }
            RequestBody::WorkspaceExecute(_)
            | RequestBody::HostRegister(_)
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

    fn workspace_list(
        &self,
        session: &BackendSession,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let workspace = self
            .workspaces
            .get(&session.workspace_id)
            .ok_or(BackendStateError::WorkspaceNotFound)?;
        Ok((
            ResponseBody::WorkspaceList(WorkspaceListResponse {
                workspaces: vec![workspace.summary.clone()],
            }),
            Vec::new(),
        ))
    }

    fn workspace_connect(
        &mut self,
        session: &BackendSession,
        workspace_id: WorkspaceId,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        self.require_workspace(session, &workspace_id)?;
        let summary = self
            .workspaces
            .get(&workspace_id)
            .ok_or(BackendStateError::WorkspaceNotFound)?
            .summary
            .clone();
        if self
            .connected_sessions
            .get(&session.session_id)
            .is_some_and(|connected| connected != &workspace_id)
        {
            return Err(BackendStateError::WorkspaceDenied);
        }
        self.connected_sessions
            .insert(session.session_id.clone(), workspace_id);
        Ok((
            ResponseBody::WorkspaceConnect(WorkspaceConnectResponse { workspace: summary }),
            Vec::new(),
        ))
    }

    fn workspace_close(
        &mut self,
        session: &BackendSession,
        workspace_id: WorkspaceId,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        self.require_workspace(session, &workspace_id)?;
        self.disconnect(&session.session_id);
        Ok((
            ResponseBody::WorkspaceClose(WorkspaceCloseResponse { workspace_id }),
            Vec::new(),
        ))
    }

    fn file_tree(
        &self,
        session: &BackendSession,
        path: Option<WorkspaceRelativePath>,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let workspace = self.workspace(session)?;
        let directory = workspace.resolver.resolve_directory(path.as_ref())?;
        let mut entries = workspace
            .files
            .list_children(&directory)?
            .into_iter()
            .map(|entry| {
                let relative = match path.as_ref() {
                    Some(parent) => format!("{}/{}", parent.as_str(), entry.name),
                    None => entry.name.clone(),
                };
                Ok(FileTreeEntry {
                    name: entry.name,
                    path: WorkspaceRelativePath::parse(relative)
                        .map_err(|_| crate::PathSecurityError::InvalidEntry)?,
                    kind: match entry.kind {
                        WorkspaceEntryKind::File => FileTreeKind::File,
                        WorkspaceEntryKind::Directory => FileTreeKind::Directory,
                        WorkspaceEntryKind::Symlink => FileTreeKind::Symlink,
                    },
                })
            })
            .collect::<Result<Vec<_>, BackendStateError>>()?;
        entries.sort_by_key(|entry| entry.name.to_lowercase());
        Ok((
            ResponseBody::FileTree(FileTreeResponse {
                workspace_id: session.workspace_id.clone(),
                path,
                entries,
            }),
            Vec::new(),
        ))
    }

    fn document_untitled(
        &mut self,
        session: &BackendSession,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let document_id = self.allocate_document_id()?;
        let workspace = self.workspace_mut(session)?;
        let resource = DocumentResource {
            document_id: document_id.clone(),
            path: None,
            document: Document::new(),
            attached_sessions: HashSet::from([session.session_id.clone()]),
        };
        let snapshot = resource.snapshot(&workspace.summary.workspace_id)?;
        workspace.documents.insert(document_id, resource);
        let event = workspace.publish(EventBody::DocumentReplaced {
            document: snapshot.clone(),
        })?;
        Ok((
            ResponseBody::DocumentUntitled(DocumentHistoryResponse { document: snapshot }),
            vec![event],
        ))
    }

    fn document_inspect(
        &mut self,
        session: &BackendSession,
        document_id: DocumentId,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let workspace = self.workspace_mut(session)?;
        let resource = workspace
            .documents
            .get_mut(&document_id)
            .ok_or(BackendStateError::DocumentNotFound)?;
        resource
            .attached_sessions
            .insert(session.session_id.clone());
        let mut snapshot = resource.snapshot(&workspace.summary.workspace_id)?;
        let missing_root = std::fs::symlink_metadata(workspace.resolver.root())
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
        if missing_root && resource.path.is_some() {
            snapshot.external_change = lapis_client_api::DocumentExternalChange::Deleted;
        } else if let Some(path) = &resource.path {
            let absolute = workspace.resolver.resolve_scoped_path(path)?;
            snapshot.external_change = match resource
                .document
                .external_change(workspace.files.fingerprint(&absolute)?.as_ref())
            {
                lapis_document::ExternalChange::Unchanged => {
                    lapis_client_api::DocumentExternalChange::Unchanged
                }
                lapis_document::ExternalChange::Modified => {
                    lapis_client_api::DocumentExternalChange::Modified
                }
                lapis_document::ExternalChange::Deleted => {
                    lapis_client_api::DocumentExternalChange::Deleted
                }
            };
        }
        Ok((
            ResponseBody::DocumentInspect(DocumentHistoryResponse { document: snapshot }),
            Vec::new(),
        ))
    }

    fn document_save_as(
        &mut self,
        session: &BackendSession,
        request: lapis_client_api::DocumentSaveAsRequest,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let absolute = self.document_target(&session.workspace_id, &request.path)?;
        let workspace = self.workspace_mut(session)?;
        let path = canonical_document_path(&workspace.resolver, &absolute)?;
        let document = workspace.require_attached_document_mut(session, &request.document_id)?;
        require_revision(
            &request.document_id,
            request.base_revision,
            document.revision(),
        )?;
        let previous = document.path.clone();
        if previous.as_ref() == Some(&path) {
            let (_, events) =
                self.document_save(session, request.document_id.clone(), request.base_revision)?;
            let (response, _) = self.document_inspect(session, request.document_id)?;
            let ResponseBody::DocumentInspect(snapshot) = response else {
                unreachable!()
            };
            return Ok((ResponseBody::DocumentSaveAs(snapshot), events));
        }
        if workspace.documents_by_path.contains_key(&absolute) {
            return Err(lapis_document::DocumentError::conflict(
                "保存先は開いている文書と重複しています",
            )
            .into());
        }
        if workspace.files.fingerprint(&absolute)?.is_some() {
            return Err(BackendStateError::Document(
                lapis_document::DocumentError::conflict("保存先は既に存在します"),
            ));
        }
        let resource = workspace
            .documents
            .get_mut(&request.document_id)
            .ok_or(BackendStateError::DocumentNotFound)?;
        let fingerprint =
            workspace
                .files
                .write_file(&absolute, &resource.document.encoded_bytes(), None)?;
        let previous_absolute = resource.document.path().map(ToOwned::to_owned);
        resource.document.mark_saved(absolute.clone(), fingerprint);
        resource.path = Some(path);
        let snapshot = resource.snapshot(&workspace.summary.workspace_id)?;
        if let Some(previous) = previous_absolute {
            workspace.documents_by_path.remove(&previous);
        }
        workspace
            .documents_by_path
            .insert(absolute, request.document_id);
        let event = workspace.publish(EventBody::DocumentReplaced {
            document: snapshot.clone(),
        })?;
        Ok((
            ResponseBody::DocumentSaveAs(DocumentHistoryResponse { document: snapshot }),
            vec![event],
        ))
    }

    fn document_reload(
        &mut self,
        session: &BackendSession,
        document_id: DocumentId,
        base_revision: Revision,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let workspace = self.workspace_mut(session)?;
        let resource = workspace.require_attached_document_mut(session, &document_id)?;
        require_revision(&document_id, base_revision, resource.revision())?;
        if resource.document.is_dirty() {
            return Err(BackendStateError::Document(
                lapis_document::DocumentError::conflict(
                    "未保存変更がある文書は再読み込みできません",
                ),
            ));
        }
        let path = resource
            .path
            .clone()
            .ok_or(BackendStateError::DocumentNotFound)?;
        let absolute = workspace.resolver.resolve_existing(&path)?;
        let data = workspace.files.read_file(&absolute)?;
        let resource = workspace
            .documents
            .get_mut(&document_id)
            .ok_or(BackendStateError::DocumentNotFound)?;
        resource.document.reload(data)?;
        let snapshot = resource.snapshot(&workspace.summary.workspace_id)?;
        let event = workspace.publish(EventBody::DocumentReplaced {
            document: snapshot.clone(),
        })?;
        Ok((
            ResponseBody::DocumentReload(DocumentHistoryResponse { document: snapshot }),
            vec![event],
        ))
    }

    fn document_discard(
        &mut self,
        session: &BackendSession,
        request: lapis_client_api::DocumentDiscardRequest,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let workspace = self.workspace_mut(session)?;
        let workspace_id = workspace.summary.workspace_id.clone();
        let resource = workspace.require_attached_document_mut(session, &request.document_id)?;
        require_revision(
            &request.document_id,
            request.base_revision,
            resource.revision(),
        )?;
        let previous_path = resource.document.path().map(ToOwned::to_owned);
        let saved_path = resource
            .document
            .saved_fingerprint()
            .and(resource.path.clone());
        let saved = saved_path
            .map(|path| {
                let absolute = workspace.resolver.resolve_existing(&path).map_err(|_| {
                    lapis_document::DocumentError::conflict(
                        "保存済み内容を読み込めないため未保存変更を保持します",
                    )
                })?;
                workspace.files.read_file(&absolute).map_err(|_| {
                    lapis_document::DocumentError::conflict(
                        "保存済み内容を読み込めないため未保存変更を保持します",
                    )
                })
            })
            .transpose()?;
        let resource = workspace.require_attached_document_mut(session, &request.document_id)?;
        resource.document.discard(saved)?;
        if resource.document.path().is_none() {
            resource.path = None;
        }
        let snapshot = resource.snapshot(&workspace_id)?;
        if snapshot.path.is_none()
            && let Some(previous) = previous_path
        {
            workspace.documents_by_path.remove(&previous);
        }
        let event = workspace.publish(EventBody::DocumentReplaced {
            document: snapshot.clone(),
        })?;
        Ok((
            ResponseBody::DocumentDiscard(lapis_client_api::DocumentDiscardResponse {
                document: snapshot,
            }),
            vec![event],
        ))
    }

    fn document_target(
        &self,
        workspace_id: &WorkspaceId,
        path: &WorkspaceRelativePath,
    ) -> Result<PathBuf, BackendStateError> {
        let workspace = self
            .workspaces
            .get(workspace_id)
            .ok_or(BackendStateError::WorkspaceNotFound)?;
        let absolute = workspace.resolver.resolve_new_file(path)?;
        if self.workspaces.iter().any(|(owner, workspace)| {
            owner != workspace_id && workspace.documents_by_path.contains_key(&absolute)
        }) {
            return Err(lapis_document::DocumentError::conflict(
                "別の Workspace が同じ Document を所有しています",
            )
            .into());
        }
        Ok(absolute)
    }

    fn document_open(
        &mut self,
        session: &BackendSession,
        path: WorkspaceRelativePath,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let allocated_document_id = self.allocate_document_id()?;
        let absolute = self.document_target(&session.workspace_id, &path)?;
        let workspace = self.workspace_mut(session)?;
        let path = canonical_document_path(&workspace.resolver, &absolute)?;
        let existing = workspace.documents_by_path.get(&absolute).cloned();
        if let Some(document_id) = existing {
            let document = workspace
                .documents
                .get_mut(&document_id)
                .ok_or(BackendStateError::DocumentNotFound)?;
            document
                .attached_sessions
                .insert(session.session_id.clone());
            let snapshot = document.snapshot(&workspace.summary.workspace_id)?;
            return Ok((
                ResponseBody::DocumentOpen(DocumentOpenResponse {
                    document: snapshot,
                    reused: true,
                }),
                Vec::new(),
            ));
        }
        let data = workspace.files.read_file(&absolute)?;
        let document = Document::from_file(absolute.clone(), data)?;
        let document_id = allocated_document_id;
        let mut resource = DocumentResource::new(document_id.clone(), path.clone(), document);
        resource
            .attached_sessions
            .insert(session.session_id.clone());
        let snapshot = resource.snapshot(&workspace.summary.workspace_id)?;
        workspace
            .documents_by_path
            .insert(absolute, document_id.clone());
        workspace.documents.insert(document_id, resource);
        let event = workspace.publish(EventBody::DocumentReplaced {
            document: snapshot.clone(),
        })?;
        Ok((
            ResponseBody::DocumentOpen(DocumentOpenResponse {
                document: snapshot,
                reused: false,
            }),
            vec![event],
        ))
    }

    fn document_create(
        &mut self,
        session: &BackendSession,
        path: WorkspaceRelativePath,
        encoding: DocumentEncoding,
        content: String,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let document_id = self.allocate_document_id()?;
        let absolute = self.document_target(&session.workspace_id, &path)?;
        let workspace = self.workspace_mut(session)?;
        let path = canonical_document_path(&workspace.resolver, &absolute)?;
        if workspace.documents_by_path.contains_key(&absolute) {
            return Err(lapis_document::DocumentError::conflict(
                "document already exists in backend state",
            )
            .into());
        }
        if workspace.files.fingerprint(&absolute)?.is_some() {
            return Err(BackendStateError::Document(
                lapis_document::DocumentError::conflict("document already exists on disk"),
            ));
        }
        let document = Document::draft(absolute.clone(), content, to_document_encoding(encoding));
        let mut resource = DocumentResource::new(document_id.clone(), path.clone(), document);
        resource
            .attached_sessions
            .insert(session.session_id.clone());
        let snapshot = resource.snapshot(&workspace.summary.workspace_id)?;
        workspace
            .documents_by_path
            .insert(absolute, document_id.clone());
        workspace.documents.insert(document_id, resource);
        let event = workspace.publish(EventBody::DocumentReplaced {
            document: snapshot.clone(),
        })?;
        Ok((
            ResponseBody::DocumentCreate(DocumentCreateResponse { document: snapshot }),
            vec![event],
        ))
    }

    fn document_edit(
        &mut self,
        session: &BackendSession,
        request: lapis_client_api::DocumentEditRequest,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let workspace = self.workspace_mut(session)?;
        let document = workspace.require_attached_document_mut(session, &request.document_id)?;
        require_revision(
            &request.document_id,
            request.base_revision,
            document.revision(),
        )?;
        let edits = request
            .transaction
            .edits()
            .iter()
            .map(|edit| {
                let start = usize::try_from(edit.start_char())
                    .map_err(|_| BackendStateError::CounterOverflow)?;
                let end = usize::try_from(edit.end_char())
                    .map_err(|_| BackendStateError::CounterOverflow)?;
                Ok(TextEdit::new(start..end, edit.replacement()))
            })
            .collect::<Result<Vec<_>, BackendStateError>>()?;
        document.document.apply_transaction(edits)?;
        let revision = document.revision();
        let event_body = EventBody::DocumentEdited {
            document_id: request.document_id.clone(),
            base_revision: request.base_revision,
            revision,
            transaction: request.transaction,
        };
        let event = workspace.publish(event_body)?;
        Ok((
            ResponseBody::DocumentEdit(DocumentEditResponse {
                document_id: request.document_id,
                revision,
            }),
            vec![event],
        ))
    }

    fn document_save(
        &mut self,
        session: &BackendSession,
        document_id: DocumentId,
        base_revision: Revision,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let workspace = self.workspace_mut(session)?;
        let files = workspace.files.clone();
        let relative = workspace
            .require_attached_document_mut(session, &document_id)?
            .path
            .clone()
            .ok_or(BackendStateError::DocumentNotFound)?;
        let path = workspace.resolver.resolve_new_file(&relative)?;
        let document = workspace.require_attached_document_mut(session, &document_id)?;
        require_revision(&document_id, base_revision, document.revision())?;
        let fingerprint = files.write_file(
            &path,
            &document.document.encoded_bytes(),
            document.document.saved_fingerprint(),
        )?;
        document.document.mark_saved(path, fingerprint);
        let revision = document.revision();
        let event = workspace.publish(EventBody::DocumentSaved {
            document_id: document_id.clone(),
            revision,
        })?;
        Ok((
            ResponseBody::DocumentSave(DocumentSaveResponse {
                document_id,
                revision,
            }),
            vec![event],
        ))
    }

    fn document_history(
        &mut self,
        session: &BackendSession,
        document_id: DocumentId,
        base_revision: Revision,
        undo: bool,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let workspace = self.workspace_mut(session)?;
        let workspace_id = workspace.summary.workspace_id.clone();
        let document = workspace.require_attached_document_mut(session, &document_id)?;
        require_revision(&document_id, base_revision, document.revision())?;
        let changed = if undo {
            document.document.undo()
        } else {
            document.document.redo()
        };
        let snapshot = document.snapshot(&workspace_id)?;
        let events = if changed {
            vec![workspace.publish(EventBody::DocumentReplaced {
                document: snapshot.clone(),
            })?]
        } else {
            Vec::new()
        };
        let response = DocumentHistoryResponse { document: snapshot };
        Ok((
            if undo {
                ResponseBody::DocumentUndo(response)
            } else {
                ResponseBody::DocumentRedo(response)
            },
            events,
        ))
    }

    fn document_close(
        &mut self,
        session: &BackendSession,
        document_id: DocumentId,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let workspace = self.workspace_mut(session)?;
        let document = workspace.require_attached_document_mut(session, &document_id)?;
        document.attached_sessions.remove(&session.session_id);
        Ok((
            ResponseBody::DocumentClose(DocumentCloseResponse { document_id }),
            Vec::new(),
        ))
    }

    fn terminal_start(
        &mut self,
        session: &BackendSession,
        request: TerminalStartRequest,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        if request.command.is_some() {
            return Err(BackendStateError::Unsupported);
        }
        let terminal_id = self.allocate_terminal_id()?;
        let workspace = self.workspace_mut(session)?;
        let cwd = workspace.resolver.resolve_directory(request.cwd.as_ref())?;
        let (terminal, event_body) = workspace.terminals.start(
            terminal_id,
            &session.session_id,
            &workspace.summary.workspace_id,
            &cwd,
            &request,
        )?;
        let event = workspace.publish(event_body)?;
        Ok((
            ResponseBody::TerminalStart(TerminalStartResponse { terminal }),
            vec![event],
        ))
    }

    fn terminal_input(
        &mut self,
        session: &BackendSession,
        request: TerminalInputRequest,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let workspace = self.workspace_mut(session)?;
        let terminal = workspace.terminals.input(
            &session.session_id,
            &workspace.summary.workspace_id,
            &request.terminal_id,
            &request.data,
        )?;
        Ok((
            ResponseBody::TerminalInput(TerminalCommandResponse { terminal }),
            Vec::new(),
        ))
    }

    fn terminal_resize(
        &mut self,
        session: &BackendSession,
        request: TerminalResizeRequest,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let workspace = self.workspace_mut(session)?;
        let terminal = workspace.terminals.resize(
            &session.session_id,
            &workspace.summary.workspace_id,
            &request.terminal_id,
            request.size,
        )?;
        Ok((
            ResponseBody::TerminalResize(TerminalCommandResponse { terminal }),
            Vec::new(),
        ))
    }

    fn terminal_terminate(
        &mut self,
        session: &BackendSession,
        request: TerminalTerminateRequest,
    ) -> Result<(ResponseBody, Vec<PublishedEvent>), BackendStateError> {
        let workspace = self.workspace_mut(session)?;
        let (terminal, event_body) = workspace.terminals.terminate(
            &session.session_id,
            &workspace.summary.workspace_id,
            &request.terminal_id,
        )?;
        let events = event_body
            .map(|event| workspace.publish(event))
            .transpose()?
            .into_iter()
            .collect();
        Ok((
            ResponseBody::TerminalTerminate(TerminalCommandResponse { terminal }),
            events,
        ))
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
            .collect::<Result<Vec<_>, BackendStateError>>()?;
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
    summary: WorkspaceSummary,
    resolver: WorkspacePathResolver,
    files: Arc<dyn WorkspaceFileBackend>,
    documents: HashMap<DocumentId, DocumentResource>,
    documents_by_path: HashMap<PathBuf, DocumentId>,
    terminals: TerminalRegistry,
    operations: Arc<Mutex<crate::operations::WorkspaceOperations>>,
    events: EventJournal,
}

impl WorkspaceState {
    fn new(registration: WorkspaceRegistration) -> Result<Self, BackendStateError> {
        Ok(Self {
            summary: WorkspaceSummary {
                workspace_id: registration.workspace_id.clone(),
                name: registration.name,
            },
            resolver: WorkspacePathResolver::new(registration.root)?,
            files: registration.files,
            documents: HashMap::new(),
            documents_by_path: HashMap::new(),
            terminals: TerminalRegistry::new(registration.terminal),
            operations: Arc::new(Mutex::new(crate::operations::WorkspaceOperations::new(
                registration.git,
                registration.tasks,
                &registration.workspace_id,
            ))),
            events: EventJournal::default(),
        })
    }

    fn require_attached_document_mut(
        &mut self,
        session: &BackendSession,
        document_id: &DocumentId,
    ) -> Result<&mut DocumentResource, BackendStateError> {
        let document = self
            .documents
            .get_mut(document_id)
            .ok_or(BackendStateError::DocumentNotFound)?;
        if !document.attached_sessions.contains(&session.session_id) {
            return Err(BackendStateError::DocumentNotAttached);
        }
        Ok(document)
    }

    fn publish(&mut self, body: EventBody) -> Result<PublishedEvent, BackendStateError> {
        let event = self.events.push(body)?;
        Ok(PublishedEvent {
            workspace_id: self.summary.workspace_id.clone(),
            event,
        })
    }
}

struct DocumentResource {
    document_id: DocumentId,
    path: Option<WorkspaceRelativePath>,
    document: Document,
    attached_sessions: HashSet<SessionId>,
}

impl DocumentResource {
    fn new(document_id: DocumentId, path: WorkspaceRelativePath, document: Document) -> Self {
        Self {
            document_id,
            path: Some(path),
            document,
            attached_sessions: HashSet::new(),
        }
    }

    fn revision(&self) -> Revision {
        Revision::new(self.document.revision().number())
    }

    fn snapshot(&self, workspace_id: &WorkspaceId) -> Result<DocumentSnapshot, BackendStateError> {
        Ok(DocumentSnapshot {
            document_id: self.document_id.clone(),
            workspace_id: workspace_id.clone(),
            path: self.path.clone(),
            content: self.document.content_for_save(),
            encoding: from_document_encoding(self.document.encoding()),
            revision: self.revision(),
            dirty: self.document.is_dirty(),
            can_undo: self.document.can_undo(),
            can_redo: self.document.can_redo(),
            external_change: lapis_client_api::DocumentExternalChange::Unchanged,
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
        && document.kind() == lapis_document::DocumentErrorKind::Conflict
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

fn to_document_encoding(encoding: DocumentEncoding) -> Encoding {
    match encoding {
        DocumentEncoding::Utf8 => Encoding::Utf8,
        DocumentEncoding::Utf8Bom => Encoding::Utf8Bom,
        DocumentEncoding::Utf16Le => Encoding::Utf16Le,
        DocumentEncoding::Utf16Be => Encoding::Utf16Be,
    }
}

fn from_document_encoding(encoding: Encoding) -> DocumentEncoding {
    match encoding {
        Encoding::Utf8 => DocumentEncoding::Utf8,
        Encoding::Utf8Bom => DocumentEncoding::Utf8Bom,
        Encoding::Utf16Le => DocumentEncoding::Utf16Le,
        Encoding::Utf16Be => DocumentEncoding::Utf16Be,
    }
}
