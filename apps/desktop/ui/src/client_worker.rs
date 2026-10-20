use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

const QUEUE_CAPACITY: usize = 64;
type Job<State> = Box<dyn FnOnce(&mut State) + Send>;

struct WorkerHandle<State> {
    sender: mpsc::SyncSender<Job<State>>,
    cancelled: Arc<AtomicBool>,
}

impl<State> Drop for WorkerHandle<State> {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

pub(crate) struct ClientWorker<State> {
    handle: Arc<WorkerHandle<State>>,
}

impl<State> Clone for ClientWorker<State> {
    fn clone(&self) -> Self {
        Self {
            handle: self.handle.clone(),
        }
    }
}

impl<State: Send + 'static> ClientWorker<State> {
    pub(crate) fn new(mut state: State) -> Self {
        let (sender, receiver) = mpsc::sync_channel::<Job<State>>(QUEUE_CAPACITY);
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        std::thread::spawn(move || {
            while let Ok(job) = receiver.recv() {
                if worker_cancelled.load(Ordering::Acquire) {
                    break;
                }
                job(&mut state);
            }
        });
        Self {
            handle: Arc::new(WorkerHandle { sender, cancelled }),
        }
    }

    pub(crate) fn request<Output: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut State) -> Result<Output, String> + Send + 'static,
    ) -> mpsc::Receiver<Result<Output, String>> {
        match self.try_request(operation) {
            Ok(receiver) => receiver,
            Err(error) => {
                let (sender, receiver) = mpsc::channel();
                let _ = sender.send(Err(error));
                receiver
            }
        }
    }

    pub(crate) fn try_request<Output: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut State) -> Result<Output, String> + Send + 'static,
    ) -> Result<mpsc::Receiver<Result<Output, String>>, String> {
        let (sender, receiver) = mpsc::channel();
        let job = Box::new(move |state: &mut State| {
            let _ = sender.send(operation(state));
        });
        if let Err(error) = self.handle.sender.try_send(job) {
            let message = match error {
                mpsc::TrySendError::Full(_) => "Client の処理待ちが上限に達しました",
                mpsc::TrySendError::Disconnected(_) => "Client worker が終了しています",
            };
            return Err(message.to_owned());
        }
        Ok(receiver)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RequestToken {
    generation: u64,
    sequence: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EditEpoch(u64);

pub(crate) struct EditChain {
    epoch: EditEpoch,
    broken: bool,
}

impl Default for EditChain {
    fn default() -> Self {
        Self {
            epoch: EditEpoch(0),
            broken: false,
        }
    }
}

impl EditChain {
    pub(crate) fn epoch(&self) -> EditEpoch {
        self.epoch
    }

    pub(crate) fn restart(&mut self) -> EditEpoch {
        self.epoch.0 = self
            .epoch
            .0
            .checked_add(1)
            .expect("Client edit epoch overflow");
        self.broken = false;
        self.epoch
    }

    pub(crate) fn apply<Output>(
        &mut self,
        epoch: EditEpoch,
        operation: impl FnOnce() -> Result<Output, String>,
    ) -> Result<Output, String> {
        if epoch != self.epoch || self.broken {
            return Err("先行する入力が失敗しました。未確定の入力を復旧してください".into());
        }
        let result = operation();
        if result.is_err() {
            self.broken = true;
        }
        result
    }
}

#[derive(Default)]
pub(crate) struct ResponseGate {
    generation: u64,
    issued: u64,
    applied: u64,
}

impl ResponseGate {
    pub(crate) fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.issued = 0;
        self.applied = 0;
    }

    pub(crate) fn issue(&mut self) -> RequestToken {
        self.issued = self
            .issued
            .checked_add(1)
            .expect("Client request sequence overflow");
        RequestToken {
            generation: self.generation,
            sequence: self.issued,
        }
    }

    pub(crate) fn accept(&mut self, token: RequestToken) -> bool {
        if token.generation != self.generation
            || token.sequence != self.issued
            || token.sequence <= self.applied
        {
            return false;
        }
        self.applied = token.sequence;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_failed_edit_invalidates_queued_ranges_even_if_remote_revision_matches_next_prediction() {
        let mut chain = EditChain::default();
        let epoch = chain.epoch();
        let mut content = "base".to_owned();
        let mut revision = 0;
        let first = chain.apply(epoch, || {
            content = "new remote content".into();
            revision = 1;
            Err::<(), _>("revision conflict".to_owned())
        });
        assert!(first.is_err());
        let second = chain.apply(epoch, || {
            assert_eq!(revision, 1);
            content.replace_range(0..1, "WRONG");
            Ok(())
        });
        assert!(second.is_err());
        assert_eq!(content, "new remote content");
        let recovered = chain.restart();
        assert!(chain.apply(epoch, || Ok(())).is_err());
        assert!(chain.apply(recovered, || Ok(())).is_ok());
    }

    #[test]
    fn queued_edit_chain_keeps_foreign_content_after_the_first_conflict() {
        let chain = EditChain::default();
        let epoch = chain.epoch();
        let worker = ClientWorker::new((chain, "base".to_owned(), 0_u64));
        let first = worker.request(move |(chain, content, revision)| {
            chain.apply(epoch, || {
                *content = "remote revision one".into();
                *revision = 1;
                Err::<(), _>("revision conflict".into())
            })
        });
        let second = worker.request(move |(chain, content, revision)| {
            chain.apply(epoch, || {
                assert_eq!(*revision, 1);
                content.replace_range(0..1, "WRONG");
                Ok(())
            })
        });
        assert!(first.recv_timeout(Duration::from_secs(2)).unwrap().is_err());
        assert!(
            second
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .is_err()
        );
        let final_state = worker.request(|(_, content, revision)| Ok((content.clone(), *revision)));
        assert_eq!(
            final_state
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap(),
            ("remote revision one".to_owned(), 1)
        );
    }

    #[test]
    fn operations_are_serial_and_execute_off_the_caller_thread() {
        let caller = std::thread::current().id();
        let worker = ClientWorker::new(Vec::new());
        let first = worker.request(move |values| {
            assert_ne!(caller, std::thread::current().id());
            values.push(1);
            Ok(())
        });
        let second = worker.request(|values| {
            values.push(2);
            Ok(values.clone())
        });
        first.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
        assert_eq!(
            second
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap(),
            vec![1, 2]
        );
    }

    #[test]
    fn full_queue_rejects_without_waiting_for_a_running_operation() {
        let worker = ClientWorker::new(());
        let (started, started_receiver) = mpsc::channel();
        let (release, release_receiver) = mpsc::channel();
        let active = worker.request(move |_| {
            started.send(()).unwrap();
            release_receiver.recv().unwrap();
            Ok(())
        });
        started_receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        let queued = (0..QUEUE_CAPACITY)
            .map(|_| worker.request(|_| Ok(())))
            .collect::<Vec<_>>();
        let rejected = worker.request(|_| Ok(()));
        assert!(rejected.try_recv().unwrap().is_err());
        release.send(()).unwrap();
        active
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        for result in queued {
            result
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .unwrap();
        }
    }

    #[test]
    fn dropping_the_last_handle_discards_backlog_without_terminating_running_resources() {
        let worker = ClientWorker::new(());
        let (started, started_receiver) = mpsc::channel();
        let (release, release_receiver) = mpsc::channel();
        let active = worker.request(move |_| {
            started.send(()).unwrap();
            release_receiver.recv().unwrap();
            Ok(())
        });
        started_receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        let executed = Arc::new(AtomicBool::new(false));
        let observed = executed.clone();
        let queued = worker.request(move |_| {
            observed.store(true, Ordering::Release);
            Ok(())
        });
        drop(worker);
        release.send(()).unwrap();
        active
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert!(queued.recv_timeout(Duration::from_secs(2)).is_err());
        assert!(!executed.load(Ordering::Acquire));
    }

    #[test]
    fn replies_cannot_roll_back_a_newer_request_or_cross_a_conversation_generation() {
        let mut gate = ResponseGate::default();
        let first = gate.issue();
        let second = gate.issue();
        assert!(!gate.accept(first));
        assert!(gate.accept(second));
        assert!(!gate.accept(first));
        assert!(!gate.accept(second));
        let previous_conversation = gate.issue();
        gate.invalidate();
        let current = gate.issue();
        assert!(!gate.accept(previous_conversation));
        assert!(gate.accept(current));
    }
}
