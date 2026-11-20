//! 既存 Desktop UI を拡張セッションの描画器に接続する。

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use terminal::{
    TerminalBackend, TerminalError, TerminalEvent, TerminalId, TerminalInput, TerminalProfileId,
    TerminalProfiles, TerminalSize, TerminalSnapshot, TerminalStatus,
};

use super::{NativeSession, NativeTransport};

struct Entry {
    session: NativeSession,
    revision: u64,
    events: Vec<TerminalEvent>,
    last_owner: Option<Option<String>>,
}

/// Workspace ごとに認証された拡張 transport を所有する。
pub struct NativeBackend {
    transport: NativeTransport,
    workspace_root: Option<PathBuf>,
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
            workspace_root: None,
            sessions: Mutex::new(HashMap::new()),
            count,
            notification_pending,
            changes: Mutex::new(Some(receiver)),
            _lifetime: lifetime,
        }
    }

    /// UI の仮想 root を Workspace 相対パスに変換する。
    pub fn for_workspace(transport: NativeTransport, root: PathBuf) -> Self {
        let mut backend = Self::new(transport);
        backend.workspace_root = Some(root);
        backend
    }

    fn relative_cwd(&self, cwd: &Path) -> Result<String, TerminalError> {
        // Local Host の root は canonicalize 済み。Windows の通常パスと \\\\?\\ パスの
        // 表記差を解消する。Remote の仮想パスは元の文字列で比較する。
        let canonical;
        let relative = match &self.workspace_root {
            Some(root) => match cwd.strip_prefix(root) {
                Ok(relative) => relative,
                Err(_) => {
                    canonical = cwd
                        .canonicalize()
                        .map_err(|_| TerminalError::new("Terminal path is outside workspace"))?;
                    canonical
                        .strip_prefix(root)
                        .map_err(|_| TerminalError::new("Terminal path is outside workspace"))?
                }
            },
            None => cwd,
        };
        if relative.components().any(|part| {
            !matches!(
                part,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        }) {
            return Err(TerminalError::new("invalid Terminal working directory"));
        }
        let value = relative
            .to_str()
            .ok_or_else(|| TerminalError::new("Terminal path is not UTF-8"))?;
        Ok(if value.is_empty() { "." } else { value }.into())
    }

    fn display_path(&self, cwd: &str) -> Result<PathBuf, TerminalError> {
        let relative = Path::new(cwd);
        if relative.components().any(|part| {
            !matches!(
                part,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        }) {
            return Err(TerminalError::new("invalid Terminal working directory"));
        }
        Ok(self
            .workspace_root
            .as_ref()
            .map_or_else(|| relative.to_owned(), |root| root.join(relative)))
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

    fn remember_session(&self, session: NativeSession) -> Result<TerminalId, TerminalError> {
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
                last_owner: None,
            },
        );
        self.count.fetch_add(1, Ordering::Release);
        Ok(id)
    }
}

fn snapshot(id: &TerminalId, entry: &Entry, cwd: PathBuf) -> TerminalSnapshot {
    TerminalSnapshot {
        id: id.clone(),
        cwd,
        status: TerminalStatus::Running,
        revision: entry.revision,
        screen: entry.session.snapshot().clone(),
    }
}

impl TerminalBackend for NativeBackend {
    fn start(&self, cwd: &Path, columns: u16, rows: u16) -> Result<TerminalId, TerminalError> {
        let cwd = self.relative_cwd(cwd)?;
        let session = NativeSession::start(
            self.transport.clone(),
            cwd,
            TerminalSize::new(columns, rows),
            None,
        )
        .map_err(TerminalError::new)?;
        self.remember_session(session)
    }

    fn start_profile(
        &self,
        cwd: &Path,
        columns: u16,
        rows: u16,
        profile: Option<&TerminalProfileId>,
    ) -> Result<TerminalId, TerminalError> {
        let cwd = self.relative_cwd(cwd)?;
        let session = NativeSession::start_profile(
            self.transport.clone(),
            cwd,
            TerminalSize::new(columns, rows),
            profile.map(|id| id.as_str().to_owned()),
        )
        .map_err(TerminalError::new)?;
        self.remember_session(session)
    }

    fn profiles(&self, _cwd: &Path) -> Result<TerminalProfiles, TerminalError> {
        NativeSession::profiles(&self.transport).map_err(TerminalError::new)
    }

    fn list(&self, _cwd: &Path) -> Result<Vec<TerminalSnapshot>, TerminalError> {
        let listed = NativeSession::list(&self.transport).map_err(TerminalError::new)?;
        listed
            .into_iter()
            .map(|session| {
                Ok(TerminalSnapshot::empty(
                    TerminalId::new(session.id),
                    self.display_path(&session.working_directory)?,
                    TerminalStatus::Running,
                    TerminalSize::new(session.columns, session.rows),
                ))
            })
            .collect::<Result<Vec<_>, TerminalError>>()
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
                    last_owner: None,
                },
            );
            self.count.fetch_add(1, Ordering::Release);
        }
        let entry = sessions.get(id.as_str()).expect("session inserted");
        Ok(snapshot(
            id,
            entry,
            self.display_path(entry.session.working_directory())?,
        ))
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
            if entry.last_owner.as_ref() != Some(&output.owner) {
                entry.last_owner = Some(output.owner.clone());
                entry.events.push(TerminalEvent::Owner {
                    owner: output.owner,
                });
            }
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
    fn translates_and_validates_workspace_paths() {
        let transport: NativeTransport = Arc::new(|_| unreachable!());
        let root = std::env::current_dir().unwrap().join("workspace");
        let backend = NativeBackend::for_workspace(transport, root.clone());
        assert_eq!(backend.relative_cwd(&root).unwrap(), ".");
        assert_eq!(backend.relative_cwd(&root.join("sub")).unwrap(), "sub");
        assert_eq!(backend.display_path("sub").unwrap(), root.join("sub"));
        assert!(
            backend
                .relative_cwd(&root.join("..").join("outside"))
                .is_err()
        );
        assert!(backend.display_path("../outside").is_err());
    }

    #[test]
    fn accepts_original_local_path_for_canonical_workspace_root() {
        let directory = tempfile::tempdir().unwrap();
        let transport: NativeTransport = Arc::new(|_| unreachable!());
        let backend =
            NativeBackend::for_workspace(transport, directory.path().canonicalize().unwrap());
        assert_eq!(backend.relative_cwd(directory.path()).unwrap(), ".");
    }

    #[test]
    fn profile_listing_and_start_use_extension_operations() {
        let operations = Arc::new(Mutex::new(Vec::new()));
        let observed = operations.clone();
        let transport: NativeTransport = Arc::new(move |request| {
            observed
                .lock()
                .unwrap()
                .push((request.operation.clone(), request.payload.clone()));
            let result = match request.operation.as_str() {
                v1::PROFILES => {
                    serde_json::json!({"profiles":[{"id":"cmd", "name":"Command Prompt", "command":{"program":"cmd.exe", "arguments":["/D"]}}], "default":"cmd"})
                }
                v1::INFO => {
                    serde_json::json!({"pty":"conpty", "default_shell":{"program":"powershell.exe", "arguments":[]}})
                }
                v1::START_PROFILE => serde_json::json!({"id":"one", "owner":"session"}),
                _ => panic!("unexpected operation {}", request.operation),
            };
            Ok(HostExtensionDispatchResponse {
                id: request.id,
                operation: request.operation,
                result: result.to_string(),
            })
        });
        let backend = NativeBackend::new(transport);
        let profiles = backend.profiles(Path::new("workspace")).unwrap();
        assert_eq!(profiles.default.unwrap().as_str(), "cmd");
        assert_eq!(profiles.profiles[0].program, "cmd.exe");
        let id = backend
            .start_profile(
                Path::new("workspace"),
                80,
                24,
                Some(&TerminalProfileId::new("cmd")),
            )
            .unwrap();
        assert_eq!(id.as_str(), "one");
        let operations = operations.lock().unwrap();
        let request: v1::StartProfileRequest = serde_json::from_str(&operations[2].1).unwrap();
        assert_eq!(operations[2].0, v1::START_PROFILE);
        assert_eq!(request.profile.as_deref(), Some("cmd"));
        assert_eq!(request.working_directory, "workspace");
    }

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
