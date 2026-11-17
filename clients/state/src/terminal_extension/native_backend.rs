//! 既存 Desktop UI を拡張セッションの描画器に接続する。

use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use terminal::{
    TerminalBackend, TerminalError, TerminalEvent, TerminalId, TerminalInput, TerminalSize,
    TerminalSnapshot, TerminalStatus,
};

use super::{NativeSession, NativeTransport};

struct Entry {
    session: NativeSession,
    revision: u64,
    events: Vec<TerminalEvent>,
}

/// Workspace ごとに認証された拡張 transport を所有する。
pub struct NativeBackend {
    transport: NativeTransport,
    sessions: Mutex<HashMap<String, Entry>>,
    count: Arc<AtomicUsize>,
    notification_pending: Arc<AtomicBool>,
    changes: Mutex<Option<futures_channel::mpsc::UnboundedReceiver<()>>>,
    _lifetime: Arc<()>,
}

impl NativeBackend {
    pub fn new(transport: NativeTransport) -> Self {
        let count = Arc::new(AtomicUsize::new(0));
        let lifetime = Arc::new(());
        let weak = Arc::downgrade(&lifetime);
        let thread_count = Arc::clone(&count);
        let notification_pending = Arc::new(AtomicBool::new(false));
        let thread_pending = Arc::clone(&notification_pending);
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
            transport,
            sessions: Mutex::new(HashMap::new()),
            count,
            notification_pending,
            changes: Mutex::new(Some(receiver)),
            _lifetime: lifetime,
        }
    }

    fn with_entry<T>(
        &self,
        id: &TerminalId,
        f: impl FnOnce(&mut Entry) -> Result<T, String>,
    ) -> Result<T, TerminalError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| TerminalError::new("Terminal state unavailable"))?;
        let entry = sessions
            .get_mut(id.as_str())
            .ok_or_else(|| TerminalError::new("Terminal session not attached"))?;
        f(entry).map_err(TerminalError::new)
    }
}

fn snapshot(id: &TerminalId, entry: &Entry) -> TerminalSnapshot {
    TerminalSnapshot {
        id: id.clone(),
        cwd: entry.session.working_directory().into(),
        status: TerminalStatus::Running,
        revision: entry.revision,
        screen: entry.session.snapshot().clone(),
    }
}

impl TerminalBackend for NativeBackend {
    fn start(&self, cwd: &Path, columns: u16, rows: u16) -> Result<TerminalId, TerminalError> {
        let cwd = cwd
            .to_str()
            .ok_or_else(|| TerminalError::new("Terminal path is not UTF-8"))?;
        let session = NativeSession::start(
            self.transport.clone(),
            cwd.into(),
            TerminalSize::new(columns, rows),
            None,
        )
        .map_err(TerminalError::new)?;
        let id = TerminalId::new(session.id());
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| TerminalError::new("Terminal state unavailable"))?;
        if sessions.contains_key(id.as_str()) {
            return Err(TerminalError::new("duplicate Terminal session ID"));
        }
        sessions.insert(
            id.as_str().to_owned(),
            Entry {
                session,
                revision: 0,
                events: Vec::new(),
            },
        );
        self.count.fetch_add(1, Ordering::Release);
        Ok(id)
    }

    fn list(&self, _cwd: &Path) -> Result<Vec<TerminalSnapshot>, TerminalError> {
        let listed = NativeSession::list(&self.transport).map_err(TerminalError::new)?;
        Ok(listed
            .into_iter()
            .map(|session| {
                TerminalSnapshot::empty(
                    TerminalId::new(session.id),
                    session.working_directory.into(),
                    TerminalStatus::Running,
                    TerminalSize::new(session.columns, session.rows),
                )
            })
            .collect())
    }

    fn attach(&self, _cwd: &Path, id: &TerminalId) -> Result<TerminalSnapshot, TerminalError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| TerminalError::new("Terminal state unavailable"))?;
        if !sessions.contains_key(id.as_str()) {
            let session = NativeSession::attach(self.transport.clone(), id.as_str())
                .map_err(TerminalError::new)?;
            sessions.insert(
                id.as_str().to_owned(),
                Entry {
                    session,
                    revision: 0,
                    events: Vec::new(),
                },
            );
            self.count.fetch_add(1, Ordering::Release);
        }
        let entry = sessions.get(id.as_str()).expect("session inserted");
        Ok(snapshot(id, entry))
    }

    fn input(&self, id: &TerminalId, input: &TerminalInput) -> Result<(), TerminalError> {
        self.with_entry(id, |entry| entry.session.input(input))
    }

    fn resize(&self, id: &TerminalId, columns: u16, rows: u16) -> Result<(), TerminalError> {
        self.with_entry(id, |entry| {
            let (_, delta) = entry.session.resize(TerminalSize::new(columns, rows))?;
            entry.revision = delta.revision;
            entry.events.push(TerminalEvent::Delta(delta));
            Ok(())
        })
    }

    fn poll(&self, id: &TerminalId) -> Result<Vec<TerminalEvent>, TerminalError> {
        self.notification_pending.store(false, Ordering::Release);
        self.with_entry(id, |entry| {
            let output = entry.session.poll(0)?;
            if let Some(delta) = output.delta {
                entry.revision = delta.revision;
                entry.events.push(TerminalEvent::Delta(delta));
            }
            entry.events.push(TerminalEvent::Owner {
                owner: output.owner,
            });
            if let Some(error) = output.read_error.or(output.write_error) {
                entry.events.push(TerminalEvent::Failed(error));
            }
            for error in entry.session.take_write_failures() {
                entry.events.push(TerminalEvent::Failed(error));
            }
            if let Some(exit) = output.exit {
                entry.events.push(TerminalEvent::Exited { code: exit.code });
            }
            Ok(std::mem::take(&mut entry.events))
        })
    }

    fn terminate(&self, id: &TerminalId) -> Result<(), TerminalError> {
        self.with_entry(id, |entry| entry.session.close())?;
        if self
            .sessions
            .lock()
            .map_err(|_| TerminalError::new("Terminal state unavailable"))?
            .remove(id.as_str())
            .is_some()
        {
            self.count.fetch_sub(1, Ordering::Release);
        }
        Ok(())
    }

    fn claim(&self, id: &TerminalId) -> Result<(), TerminalError> {
        self.with_entry(id, |entry| entry.session.claim().map(|_| ()))
    }

    fn release(&self, id: &TerminalId) -> Result<(), TerminalError> {
        self.with_entry(id, |entry| entry.session.release().map(|_| ()))
    }

    fn changes(&self) -> Option<futures_channel::mpsc::UnboundedReceiver<()>> {
        self.changes.lock().ok()?.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_extension::v1;
    use client_api::HostExtensionDispatchResponse;
    use std::sync::Arc;

    #[test]
    fn adapts_extension_output_and_keeps_resize_order() {
        let transport: NativeTransport = Arc::new(|request| {
            let result = match request.operation.as_str() {
                v1::INFO => {
                    serde_json::json!({"pty":"posix","default_shell":{"program":"sh","arguments":[]}})
                }
                v1::START => serde_json::json!({"id":"one","owner":"session"}),
                v1::READ => {
                    serde_json::json!({"bytes":[104,105],"next":2,"lost_before":null,"exit":null,"read_error":null,"write_error":null,"owner":"session"})
                }
                v1::RESIZE | v1::CLOSE => serde_json::Value::Null,
                _ => panic!("unexpected operation {}", request.operation),
            };
            Ok(HostExtensionDispatchResponse {
                id: request.id,
                operation: request.operation,
                result: result.to_string(),
            })
        });
        let backend = NativeBackend::new(transport);
        let id = backend.start(Path::new("workspace"), 20, 3).unwrap();
        assert_eq!(
            backend
                .attach(Path::new("workspace"), &id)
                .unwrap()
                .revision,
            0
        );
        backend.resize(&id, 30, 4).unwrap();
        let events = backend.poll(&id).unwrap();
        let revisions: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                TerminalEvent::Delta(delta) => Some(delta.revision),
                _ => None,
            })
            .collect();
        assert_eq!(revisions, [1, 2]);
        let snapshot = backend.attach(Path::new("workspace"), &id).unwrap();
        assert_eq!(snapshot.revision, 2);
        assert!(snapshot.screen.plain_text().contains("hi"));
        backend.terminate(&id).unwrap();
    }
}
