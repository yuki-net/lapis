use std::{sync::mpsc, time::Duration};

use lapis_client_api::EventEnvelope;

pub struct BackendEventReceiver {
    pub(crate) receiver: mpsc::Receiver<EventEnvelope>,
}

impl BackendEventReceiver {
    pub fn recv_timeout(&self, timeout: Duration) -> Result<EventEnvelope, mpsc::RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }

    pub fn try_recv(&self) -> Result<EventEnvelope, mpsc::TryRecvError> {
        self.receiver.try_recv()
    }
}
