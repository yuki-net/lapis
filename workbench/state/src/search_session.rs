use std::{path::PathBuf, sync::Arc, sync::atomic::AtomicBool};
use std::{sync::atomic::Ordering, sync::mpsc, thread};
use workspace::{SearchError, SearchHit, WorkspaceSearchBackend};

pub struct WorkspaceSearchSession {
    backend: Arc<dyn WorkspaceSearchBackend>,
    cancelled: Option<Arc<AtomicBool>>,
    receiver: Option<mpsc::Receiver<Result<Vec<SearchHit>, SearchError>>>,
    query: String,
    hits: Vec<SearchHit>,
    running: bool,
    error: Option<String>,
}

impl WorkspaceSearchSession {
    pub fn new(backend: Arc<dyn WorkspaceSearchBackend>) -> Self {
        Self {
            backend,
            cancelled: None,
            receiver: None,
            query: String::new(),
            hits: Vec::new(),
            running: false,
            error: None,
        }
    }

    pub fn start(&mut self, root: PathBuf, query: String) {
        self.cancel();
        self.query = query.clone();
        self.hits.clear();
        self.error = None;
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        let backend = self.backend.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = backend.search(&root, &query, &worker_cancelled);
            let _ = sender.send(result);
        });
        self.cancelled = Some(cancelled);
        self.receiver = Some(receiver);
        self.running = true;
    }

    /// 完了通知用の receiver を UI へ渡す。渡した後は `refresh` では結果を取得しない。
    pub fn take_receiver(&mut self) -> Option<mpsc::Receiver<Result<Vec<SearchHit>, SearchError>>> {
        self.receiver.take()
    }

    pub fn cancel(&mut self) {
        if let Some(cancelled) = self.cancelled.take() {
            cancelled.store(true, Ordering::Relaxed);
        }
        self.receiver = None;
        self.running = false;
    }

    pub fn refresh(&mut self) -> bool {
        let Some(receiver) = &self.receiver else {
            return false;
        };
        let Ok(result) = receiver.try_recv() else {
            return false;
        };
        match result {
            Ok(hits) => self.hits = hits,
            Err(error) => self.error = Some(error.to_string()),
        }
        self.receiver = None;
        self.cancelled = None;
        self.running = false;
        true
    }

    pub fn query(&self) -> &str {
        &self.query
    }
    pub fn hits(&self) -> &[SearchHit] {
        &self.hits
    }
    pub fn is_running(&self) -> bool {
        self.running
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn backend(&self) -> Arc<dyn WorkspaceSearchBackend> {
        self.backend.clone()
    }
}
