//! Workspace ごとの拡張接続を Desktop のターミナル画面へつなぐ。

use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use client_api::WorkspaceId;
use desktop_client::terminal_extension::{NativeBackend, NativeTransport};
use host::BackendClient;
use terminal::{
    TerminalBackend, TerminalError, TerminalEvent, TerminalId, TerminalInput, TerminalSnapshot,
};

pub(crate) struct ExtensionTerminals {
    host: BackendClient,
    workspaces: Mutex<HashMap<WorkspaceId, Arc<NativeBackend>>>,
    sessions: Mutex<HashMap<String, Arc<NativeBackend>>>,
    count: Arc<AtomicUsize>,
    pending: Arc<AtomicBool>,
    changes: Mutex<Option<futures_channel::mpsc::UnboundedReceiver<()>>>,
    _lifetime: Arc<()>,
}

impl ExtensionTerminals {
    pub(crate) fn new(host: BackendClient) -> Self {
        let count = Arc::new(AtomicUsize::new(0));
        let pending = Arc::new(AtomicBool::new(false));
        let lifetime = Arc::new(());
        let weak = Arc::downgrade(&lifetime);
        let thread_count = Arc::clone(&count);
        let thread_pending = Arc::clone(&pending);
        let (sender, receiver) = futures_channel::mpsc::unbounded();
        std::thread::spawn(move || {
            while weak.upgrade().is_some() {
                if thread_count.load(Ordering::Acquire) > 0
                    && !thread_pending.swap(true, Ordering::AcqRel)
                    && sender.unbounded_send(()).is_err()
                {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        Self {
            host,
            workspaces: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            count,
            pending,
            changes: Mutex::new(Some(receiver)),
            _lifetime: lifetime,
        }
    }

    fn workspace(&self, cwd: &Path) -> Result<Arc<NativeBackend>, TerminalError> {
        let (root, workspace) = self.host.workspace_for_directory(cwd).map_err(error)?;
        let mut entries = self.workspaces.lock().map_err(error)?;
        if let Some(backend) = entries.get(&workspace) {
            return Ok(Arc::clone(backend));
        }
        let connection = self
            .host
            .open_extension_connection(workspace.clone())
            .map_err(error)?;
        let transport: NativeTransport = Arc::new(move |request| {
            connection
                .dispatch(request)
                .map_err(|error| error.to_string())
        });
        let backend = Arc::new(NativeBackend::for_workspace(transport, root));
        entries.insert(workspace, Arc::clone(&backend));
        Ok(backend)
    }

    fn remember(&self, id: &TerminalId, backend: &Arc<NativeBackend>) -> Result<(), TerminalError> {
        let mut sessions = self.sessions.lock().map_err(error)?;
        if let Some(existing) = sessions.get(id.as_str()) {
            if !Arc::ptr_eq(existing, backend) {
                return Err(TerminalError::new("Terminal session ID collision"));
            }
        } else {
            sessions.insert(id.as_str().to_owned(), Arc::clone(backend));
            self.count.fetch_add(1, Ordering::Release);
        }
        Ok(())
    }

    fn session(&self, id: &TerminalId) -> Result<Arc<NativeBackend>, TerminalError> {
        self.sessions
            .lock()
            .map_err(error)?
            .get(id.as_str())
            .cloned()
            .ok_or_else(|| TerminalError::new("Terminal session not attached"))
    }
}

fn error(value: impl std::fmt::Display) -> TerminalError {
    TerminalError::new(value.to_string())
}

impl TerminalBackend for ExtensionTerminals {
    fn start(&self, cwd: &Path, columns: u16, rows: u16) -> Result<TerminalId, TerminalError> {
        let backend = self.workspace(cwd)?;
        let id = backend.start(cwd, columns, rows)?;
        self.remember(&id, &backend)?;
        Ok(id)
    }

    fn list(&self, cwd: &Path) -> Result<Vec<TerminalSnapshot>, TerminalError> {
        let backend = self.workspace(cwd)?;
        let sessions = backend.list(cwd)?;
        for session in &sessions {
            self.remember(&session.id, &backend)?;
        }
        Ok(sessions)
    }

    fn attach(&self, cwd: &Path, id: &TerminalId) -> Result<TerminalSnapshot, TerminalError> {
        let backend = self.workspace(cwd)?;
        let snapshot = backend.attach(cwd, id)?;
        self.remember(id, &backend)?;
        Ok(snapshot)
    }

    fn input(&self, id: &TerminalId, input: &TerminalInput) -> Result<(), TerminalError> {
        self.session(id)?.input(id, input)
    }
    fn resize(&self, id: &TerminalId, columns: u16, rows: u16) -> Result<(), TerminalError> {
        self.session(id)?.resize(id, columns, rows)
    }
    fn poll(&self, id: &TerminalId) -> Result<Vec<TerminalEvent>, TerminalError> {
        self.pending.store(false, Ordering::Release);
        self.session(id)?.poll(id)
    }
    fn terminate(&self, id: &TerminalId) -> Result<(), TerminalError> {
        self.session(id)?.terminate(id)?;
        if self
            .sessions
            .lock()
            .map_err(error)?
            .remove(id.as_str())
            .is_some()
        {
            self.count.fetch_sub(1, Ordering::Release);
        }
        Ok(())
    }
    fn claim(&self, id: &TerminalId) -> Result<(), TerminalError> {
        self.session(id)?.claim(id)
    }
    fn release(&self, id: &TerminalId) -> Result<(), TerminalError> {
        self.session(id)?.release(id)
    }
    fn changes(&self) -> Option<futures_channel::mpsc::UnboundedReceiver<()>> {
        self.changes.lock().ok()?.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspaces_reuse_connections_and_keep_session_routes_separate() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let nested = first.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let host = host::BackendHost::start().unwrap();
        host.register_workspace(first.path()).unwrap();
        host.register_workspace(second.path()).unwrap();
        let backend = ExtensionTerminals::new(BackendClient::from(host));
        let one = backend.workspace(first.path()).unwrap();
        let nested_backend = backend.workspace(&nested).unwrap();
        let two = backend.workspace(second.path()).unwrap();
        assert!(Arc::ptr_eq(&one, &nested_backend));
        assert!(!Arc::ptr_eq(&one, &two));
        let id = TerminalId::new("session-one");
        backend.remember(&id, &one).unwrap();
        backend.remember(&id, &one).unwrap();
        assert!(Arc::ptr_eq(&backend.session(&id).unwrap(), &one));
        assert!(backend.remember(&id, &two).is_err());
        assert_eq!(backend.count.load(Ordering::Acquire), 1);
    }
}
