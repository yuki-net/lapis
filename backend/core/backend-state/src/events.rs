use std::{sync::mpsc, time::Duration};

use lapis_client_api::EventEnvelope;

pub trait BackendEventSink: Send + Sync + 'static {
    /// falseは購読を継続できず、Snapshot再同期が必要であることを表す。
    fn try_send(&self, event: EventEnvelope) -> bool;
}

pub struct BackendEventReceiver {
    pub(crate) receiver: mpsc::Receiver<EventEnvelope>,
}

impl BackendEventReceiver {
    pub fn recv(&self) -> Result<EventEnvelope, mpsc::RecvError> {
        self.receiver.recv()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<EventEnvelope, mpsc::RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }

    pub fn try_recv(&self) -> Result<EventEnvelope, mpsc::TryRecvError> {
        self.receiver.try_recv()
    }
}
