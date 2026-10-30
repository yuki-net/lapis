use std::{
    collections::HashMap,
    error::Error,
    fmt,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

use lapis_client_api::{
    EventEnvelope, RequestEnvelope, ResponseBody, ResponseEnvelope, SessionId, WorkspaceId,
};

use crate::{
    BackendEventReceiver, BackendEventSink, BackendSession, BackendState, BackendStateError,
    WorkspaceRegistration, state::PublishedEvent,
};

const TERMINAL_POLL_INTERVAL: Duration = Duration::from_millis(25);
const SUBSCRIBER_QUEUE_CAPACITY: usize = 256;

#[derive(Clone)]
pub struct BackendService {
    inner: Arc<ServiceInner>,
}

impl BackendService {
    /// Host内部の登録用。Git I/Oは正規状態workerの外で実行する。
    pub fn worktree_path(
        &self,
        workspace_id: WorkspaceId,
        worktree: lapis_editor_core::TaskId,
    ) -> Result<std::path::PathBuf, BackendServiceError> {
        let (respond, receive) = mpsc::sync_channel(1);
        self.inner
            .sender()?
            .send(Command::PrepareWorktreeLookup {
                workspace_id,
                worktree,
                respond,
            })
            .map_err(|_| BackendServiceError::Unavailable)?;
        receive
            .recv()
            .map_err(|_| BackendServiceError::Unavailable)?
            .and_then(|lookup| lookup.execute())
            .map_err(BackendServiceError::State)
    }
    pub fn relocate_document(
        &self,
        source: WorkspaceId,
        destination: WorkspaceId,
        document_id: lapis_client_api::DocumentId,
        revision: lapis_client_api::Revision,
        path: lapis_client_api::WorkspaceRelativePath,
    ) -> Result<(), BackendServiceError> {
        let (respond, receive) = mpsc::sync_channel(1);
        self.inner
            .sender()?
            .send(Command::RelocateDocument {
                source,
                destination,
                document_id,
                revision,
                path,
                respond,
            })
            .map_err(|_| BackendServiceError::Unavailable)?;
        receive
            .recv()
            .map_err(|_| BackendServiceError::Unavailable)?
            .map_err(BackendServiceError::State)
    }
    pub fn start(mut state: BackendState) -> Result<Self, BackendServiceError> {
        let (sender, receiver) = mpsc::sync_channel(64);
        let git_sender = sender.clone();
        let git_publish: Arc<dyn Fn(WorkspaceId, String, lapis_git::GitSnapshot) + Send + Sync> =
            Arc::new(move |workspace_id, repository_id, status| {
                let _ = git_sender.send(Command::GitStatusChanged {
                    workspace_id,
                    repository_id,
                    status,
                });
            });
        state.start_git_updates(git_publish.clone());
        let thread = thread::Builder::new()
            .name("lapis-backend-state".to_owned())
            .spawn(move || run_worker(state, receiver, git_publish))
            .map_err(BackendServiceError::ThreadSpawn)?;
        Ok(Self {
            inner: Arc::new(ServiceInner {
                sender: Mutex::new(Some(sender)),
                thread: Mutex::new(Some(thread)),
            }),
        })
    }

    pub fn register_workspace(
        &self,
        registration: WorkspaceRegistration,
    ) -> Result<(), BackendServiceError> {
        let (respond, receive) = mpsc::sync_channel(1);
        self.inner
            .sender()?
            .send(Command::RegisterWorkspace {
                registration,
                respond,
            })
            .map_err(|_| BackendServiceError::Unavailable)?;
        receive
            .recv()
            .map_err(|_| BackendServiceError::Unavailable)?
            .map_err(BackendServiceError::State)
    }

    pub fn dispatch(
        &self,
        session: BackendSession,
        request: RequestEnvelope,
    ) -> Result<ResponseEnvelope, BackendServiceError> {
        if let lapis_client_api::RequestBody::WorkspaceExecute(operation) = request.body {
            return self.execute_workspace_operation(session, request.request_id, operation);
        }
        let (respond, receive) = mpsc::sync_channel(1);
        self.inner
            .sender()?
            .send(Command::Dispatch {
                session,
                request,
                respond,
            })
            .map_err(|_| BackendServiceError::Unavailable)?;
        receive.recv().map_err(|_| BackendServiceError::Unavailable)
    }

    fn execute_workspace_operation(
        &self,
        session: BackendSession,
        request_id: lapis_client_api::RequestId,
        request: lapis_client_api::WorkspaceOperationRequest,
    ) -> Result<ResponseEnvelope, BackendServiceError> {
        let changed = request.operation.is_mutation();
        let workspace_id = session.workspace_id.clone();
        let removing = match &request.operation {
            lapis_client_api::WorkspaceOperation::DiscardWorktree { worktree_id } => {
                Some(worktree_id.clone())
            }
            _ => None,
        };
        let (respond, receive) = mpsc::sync_channel(1);
        self.inner
            .sender()?
            .send(Command::PrepareWorkspaceOperation {
                session,
                request,
                respond,
            })
            .map_err(|_| BackendServiceError::Unavailable)?;
        let prepared = receive
            .recv()
            .map_err(|_| BackendServiceError::Unavailable)?;
        let mut reserved = None;
        let admitted = prepared.is_ok();
        let result = prepared.and_then(|operation| {
            if let Some(id) = removing {
                let root =
                    self.worktree_path(workspace_id.clone(), id)
                        .map_err(|error| match error {
                            BackendServiceError::State(error) => error,
                            error => BackendStateError::Operation(error.to_string()),
                        })?;
                let (respond, receive) = mpsc::sync_channel(1);
                self.inner
                    .sender()
                    .map_err(|error| BackendStateError::Operation(error.to_string()))?
                    .send(Command::ReserveWorktreeRemoval {
                        root: root.clone(),
                        respond,
                    })
                    .map_err(|_| BackendStateError::Operation("Backend unavailable".into()))?;
                receive
                    .recv()
                    .map_err(|_| BackendStateError::Operation("Backend unavailable".into()))??;
                reserved = Some(root);
            }
            operation.execute()
        });
        let (respond, receive) = mpsc::sync_channel(1);
        self.inner
            .sender()?
            .send(Command::CompleteWorkspaceOperation {
                workspace_id,
                changed,
                request_id,
                result,
                reserved,
                admitted,
                respond,
            })
            .map_err(|_| BackendServiceError::Unavailable)?;
        receive.recv().map_err(|_| BackendServiceError::Unavailable)
    }

    pub fn subscribe(
        &self,
        session: BackendSession,
    ) -> Result<BackendEventReceiver, BackendServiceError> {
        let (respond, receive) = mpsc::sync_channel(1);
        self.inner
            .sender()?
            .send(Command::Subscribe { session, respond })
            .map_err(|_| BackendServiceError::Unavailable)?;
        receive
            .recv()
            .map_err(|_| BackendServiceError::Unavailable)?
            .map_err(BackendServiceError::State)
    }

    pub fn dispatch_with_subscription(
        &self,
        session: BackendSession,
        request: RequestEnvelope,
        sink: Arc<dyn BackendEventSink>,
    ) -> Result<ResponseEnvelope, BackendServiceError> {
        if matches!(
            request.body,
            lapis_client_api::RequestBody::WorkspaceExecute(_)
        ) {
            return self.dispatch(session, request);
        }
        let (respond, receive) = mpsc::sync_channel(1);
        self.inner
            .sender()?
            .send(Command::DispatchWithSubscription {
                session,
                request,
                sink,
                respond,
            })
            .map_err(|_| BackendServiceError::Unavailable)?;
        receive.recv().map_err(|_| BackendServiceError::Unavailable)
    }

    pub fn disconnect(&self, session_id: SessionId) -> Result<(), BackendServiceError> {
        self.inner
            .sender()?
            .send(Command::Disconnect(session_id))
            .map_err(|_| BackendServiceError::Unavailable)
    }
}

struct ServiceInner {
    sender: Mutex<Option<mpsc::SyncSender<Command>>>,
    thread: Mutex<Option<thread::JoinHandle<()>>>,
}

impl ServiceInner {
    fn sender(&self) -> Result<mpsc::SyncSender<Command>, BackendServiceError> {
        self.sender
            .lock()
            .map_err(|_| BackendServiceError::Unavailable)?
            .clone()
            .ok_or(BackendServiceError::Unavailable)
    }
}

impl Drop for ServiceInner {
    fn drop(&mut self) {
        if let Ok(sender) = self.sender.get_mut()
            && let Some(sender) = sender.take()
        {
            let _ = sender.send(Command::Shutdown);
            drop(sender);
        }
        if let Ok(thread) = self.thread.get_mut()
            && let Some(thread) = thread.take()
        {
            let _ = thread.join();
        }
    }
}

enum Command {
    GitStatusChanged {
        workspace_id: WorkspaceId,
        repository_id: String,
        status: lapis_git::GitSnapshot,
    },
    ReserveWorktreeRemoval {
        root: std::path::PathBuf,
        respond: mpsc::SyncSender<Result<(), BackendStateError>>,
    },
    PrepareWorktreeLookup {
        workspace_id: WorkspaceId,
        worktree: lapis_editor_core::TaskId,
        respond: mpsc::SyncSender<Result<crate::state::PreparedWorktreeLookup, BackendStateError>>,
    },
    PrepareWorkspaceOperation {
        session: BackendSession,
        request: lapis_client_api::WorkspaceOperationRequest,
        respond:
            mpsc::SyncSender<Result<crate::state::PreparedWorkspaceOperation, BackendStateError>>,
    },
    CompleteWorkspaceOperation {
        workspace_id: WorkspaceId,
        changed: bool,
        request_id: lapis_client_api::RequestId,
        result: Result<lapis_client_api::WorkspaceOperationResult, BackendStateError>,
        reserved: Option<std::path::PathBuf>,
        admitted: bool,
        respond: mpsc::SyncSender<ResponseEnvelope>,
    },
    RelocateDocument {
        source: WorkspaceId,
        destination: WorkspaceId,
        document_id: lapis_client_api::DocumentId,
        revision: lapis_client_api::Revision,
        path: lapis_client_api::WorkspaceRelativePath,
        respond: mpsc::SyncSender<Result<(), BackendStateError>>,
    },
    RegisterWorkspace {
        registration: WorkspaceRegistration,
        respond: mpsc::SyncSender<Result<(), BackendStateError>>,
    },
    Dispatch {
        session: BackendSession,
        request: RequestEnvelope,
        respond: mpsc::SyncSender<ResponseEnvelope>,
    },
    DispatchWithSubscription {
        session: BackendSession,
        request: RequestEnvelope,
        sink: Arc<dyn BackendEventSink>,
        respond: mpsc::SyncSender<ResponseEnvelope>,
    },
    Subscribe {
        session: BackendSession,
        respond: mpsc::SyncSender<Result<BackendEventReceiver, BackendStateError>>,
    },
    Disconnect(SessionId),
    Shutdown,
}

struct Subscriber {
    workspace_id: WorkspaceId,
    sink: Arc<dyn BackendEventSink>,
}

struct ChannelEventSink {
    sender: mpsc::SyncSender<EventEnvelope>,
}

impl BackendEventSink for ChannelEventSink {
    fn try_send(&self, event: EventEnvelope) -> bool {
        self.sender.try_send(event).is_ok()
    }
}

fn run_worker(
    mut state: BackendState,
    receiver: mpsc::Receiver<Command>,
    git_publish: Arc<dyn Fn(WorkspaceId, String, lapis_git::GitSnapshot) + Send + Sync>,
) {
    let mut subscribers = HashMap::<SessionId, Subscriber>::new();
    loop {
        match receiver.recv_timeout(TERMINAL_POLL_INTERVAL) {
            Ok(Command::GitStatusChanged {
                workspace_id,
                repository_id,
                status,
            }) => {
                if let Ok(Some(event)) =
                    state.publish_git_status(workspace_id, repository_id, status)
                {
                    publish_events(&mut subscribers, vec![event]);
                }
            }
            Ok(Command::ReserveWorktreeRemoval { root, respond }) => {
                let _ = respond.send(state.reserve_worktree_removal(root));
            }
            Ok(Command::PrepareWorktreeLookup {
                workspace_id,
                worktree,
                respond,
            }) => {
                let _ = respond.send(state.prepare_worktree_lookup(&workspace_id, worktree));
            }
            Ok(Command::PrepareWorkspaceOperation {
                session,
                request,
                respond,
            }) => {
                let _ = respond.send(state.prepare_workspace_operation(&session, request));
            }
            Ok(Command::CompleteWorkspaceOperation {
                workspace_id,
                changed,
                request_id,
                result,
                reserved,
                admitted,
                respond,
            }) => {
                if admitted {
                    state.finish_workspace_operation(&workspace_id);
                }
                if let Some(root) = reserved {
                    state.release_worktree_removal(&root);
                }
                let outcome =
                    state.complete_workspace_operation(workspace_id, changed, request_id, result);
                let _ = respond.send(outcome.response);
                publish_events(&mut subscribers, outcome.events);
            }
            Ok(Command::RelocateDocument {
                source,
                destination,
                document_id,
                revision,
                path,
                respond,
            }) => {
                let result =
                    state.relocate_document(source, destination, document_id, revision, path);
                match result {
                    Ok(events) => {
                        publish_events(&mut subscribers, events);
                        let _ = respond.send(Ok(()));
                    }
                    Err(error) => {
                        let _ = respond.send(Err(error));
                    }
                }
            }
            Ok(Command::RegisterWorkspace {
                registration,
                respond,
            }) => {
                let result = state.register_workspace(registration);
                if result.is_ok() {
                    state.start_git_updates(git_publish.clone());
                }
                let _ = respond.send(result);
            }
            Ok(Command::Dispatch {
                session,
                request,
                respond,
            }) => {
                let outcome = state.dispatch(&session, request);
                if !state.has_connected_session(&session.session_id) {
                    subscribers.remove(&session.session_id);
                }
                let _ = respond.send(outcome.response);
                publish_events(&mut subscribers, outcome.events);
            }
            Ok(Command::DispatchWithSubscription {
                session,
                request,
                sink,
                respond,
            }) => {
                let outcome = state.dispatch(&session, request);
                if matches!(outcome.response.body, ResponseBody::WorkspaceConnect(_)) {
                    subscribers.insert(
                        session.session_id.clone(),
                        Subscriber {
                            workspace_id: session.workspace_id.clone(),
                            sink,
                        },
                    );
                } else if !state.has_connected_session(&session.session_id) {
                    subscribers.remove(&session.session_id);
                }
                let _ = respond.send(outcome.response);
                publish_events(&mut subscribers, outcome.events);
            }
            Ok(Command::Subscribe { session, respond }) => {
                if let Err(error) = state.require_connected(&session) {
                    let _ = respond.send(Err(error));
                    continue;
                }
                let (sender, receiver) = mpsc::sync_channel(SUBSCRIBER_QUEUE_CAPACITY);
                subscribers.insert(
                    session.session_id,
                    Subscriber {
                        workspace_id: session.workspace_id,
                        sink: Arc::new(ChannelEventSink { sender }),
                    },
                );
                let _ = respond.send(Ok(BackendEventReceiver { receiver }));
            }
            Ok(Command::Disconnect(session_id)) => {
                state.disconnect(&session_id);
                subscribers.remove(&session_id);
            }
            Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if let Ok(events) = state.poll_terminals() {
            publish_events(&mut subscribers, events);
        }
    }
}

fn publish_events(subscribers: &mut HashMap<SessionId, Subscriber>, events: Vec<PublishedEvent>) {
    for published in events {
        subscribers.retain(|_, subscriber| {
            if subscriber.workspace_id != published.workspace_id {
                return true;
            }
            subscriber.sink.try_send(published.event.clone())
        });
    }
}

#[derive(Debug)]
pub enum BackendServiceError {
    ThreadSpawn(std::io::Error),
    State(BackendStateError),
    Rejected(lapis_client_api::ProtocolError),
    Connection(String),
    Unavailable,
}

impl fmt::Display for BackendServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ThreadSpawn(error) => {
                write!(formatter, "failed to start backend worker: {error}")
            }
            Self::State(error) => write!(formatter, "backend state rejected operation: {error}"),
            Self::Rejected(error) => {
                write!(
                    formatter,
                    "backend rejected request: {}",
                    error.detail.as_deref().unwrap_or(error.code.as_str())
                )
            }
            Self::Unavailable => formatter.write_str("backend worker is unavailable"),
            Self::Connection(detail) => write!(formatter, "Host接続: {detail}"),
        }
    }
}

impl Error for BackendServiceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ThreadSpawn(error) => Some(error),
            Self::State(error) => Some(error),
            Self::Unavailable | Self::Rejected(_) | Self::Connection(_) => None,
        }
    }
}
