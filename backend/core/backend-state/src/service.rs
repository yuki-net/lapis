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
    state::PublishedEvent,
};

const TERMINAL_POLL_INTERVAL: Duration = Duration::from_millis(25);
const SUBSCRIBER_QUEUE_CAPACITY: usize = 256;

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

    pub fn dispatch_with_subscription(
        &self,
        session: BackendSession,
        request: RequestEnvelope,
        sink: Arc<dyn BackendEventSink>,
    ) -> Result<ResponseEnvelope, BackendServiceError> {
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

fn run_worker(mut state: BackendState, receiver: mpsc::Receiver<Command>) {
    let mut subscribers = HashMap::<SessionId, Subscriber>::new();
    loop {
        match receiver.recv_timeout(TERMINAL_POLL_INTERVAL) {
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
