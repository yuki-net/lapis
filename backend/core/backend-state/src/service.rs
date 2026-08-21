use std::{
    collections::HashMap,
    error::Error,
    fmt,
    sync::{Arc, Mutex, mpsc},
    thread,
};

use lapis_client_api::{EventEnvelope, RequestEnvelope, ResponseEnvelope, SessionId, WorkspaceId};

use crate::{BackendEventReceiver, BackendSession, BackendState, BackendStateError};

#[derive(Clone)]
pub struct BackendService {
    inner: Arc<ServiceInner>,
}

impl BackendService {
    pub fn start(state: BackendState) -> Result<Self, BackendServiceError> {
        let (sender, receiver) = mpsc::sync_channel(64);
        let thread = thread::Builder::new()
            .name("lapis-backend-state".to_owned())
            .spawn(move || run_worker(state, receiver))
            .map_err(BackendServiceError::ThreadSpawn)?;
        Ok(Self {
            inner: Arc::new(ServiceInner {
                sender: Mutex::new(Some(sender)),
                thread: Mutex::new(Some(thread)),
            }),
        })
    }

    pub fn dispatch(
        &self,
        session: BackendSession,
        request: RequestEnvelope,
    ) -> Result<ResponseEnvelope, BackendServiceError> {
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
            let _ = sender.try_send(Command::Shutdown);
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
    Dispatch {
        session: BackendSession,
        request: RequestEnvelope,
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
    sender: mpsc::Sender<EventEnvelope>,
}

fn run_worker(mut state: BackendState, receiver: mpsc::Receiver<Command>) {
    let mut subscribers = HashMap::<SessionId, Subscriber>::new();
    while let Ok(command) = receiver.recv() {
        match command {
            Command::Dispatch {
                session,
                request,
                respond,
            } => {
                let outcome = state.dispatch(&session, request);
                let _ = respond.send(outcome.response);
                for published in outcome.events {
                    subscribers.retain(|_, subscriber| {
                        if subscriber.workspace_id != published.workspace_id {
                            return true;
                        }
                        subscriber.sender.send(published.event.clone()).is_ok()
                    });
                }
            }
            Command::Subscribe { session, respond } => {
                if let Err(error) = state.require_connected(&session) {
                    let _ = respond.send(Err(error));
                    continue;
                }
                let (sender, receiver) = mpsc::channel();
                subscribers.insert(
                    session.session_id,
                    Subscriber {
                        workspace_id: session.workspace_id,
                        sender,
                    },
                );
                let _ = respond.send(Ok(BackendEventReceiver { receiver }));
            }
            Command::Disconnect(session_id) => {
                state.disconnect(&session_id);
                subscribers.remove(&session_id);
            }
            Command::Shutdown => break,
        }
    }
}

#[derive(Debug)]
pub enum BackendServiceError {
    ThreadSpawn(std::io::Error),
    State(BackendStateError),
    Unavailable,
}

impl fmt::Display for BackendServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ThreadSpawn(error) => {
                write!(formatter, "failed to start backend worker: {error}")
            }
            Self::State(error) => write!(formatter, "backend state rejected operation: {error}"),
            Self::Unavailable => formatter.write_str("backend worker is unavailable"),
        }
    }
}

impl Error for BackendServiceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ThreadSpawn(error) => Some(error),
            Self::State(error) => Some(error),
            Self::Unavailable => None,
        }
    }
}
