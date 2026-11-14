//! Terminal 拡張の Host 側状態サービス。
//!
//! Terminal の正規状態・画面差分・接続 Session の正本はこの crate
//! （`extensions/terminal/host`）だけ。PTY の具体実装は `extensions/terminal/adapter` が持つ。

use std::{
    collections::{BTreeMap, HashSet},
    error::Error,
    fmt,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use client_api::{
    EventBody, SessionId, SnapshotReason, TerminalId, TerminalInput, TerminalProfiles,
    TerminalSize, TerminalSnapshot, TerminalStartRequest, TerminalStatus, WorkspaceId,
    WorkspaceRelativePath,
};
use terminal::{
    TerminalBackend, TerminalDelta, TerminalEvent, TerminalId as BackendTerminalId, TerminalScreen,
    TerminalStatus as BackendTerminalStatus,
};

/// Terminal 状態の操作で発生するエラー。
#[derive(Debug)]
pub enum TerminalStateError {
    Unsupported,
    NotFound,
    NotAttached,
    NotRunning,
    NotOwner,
    InvalidSize,
    UnknownProfile,
    Backend(terminal::TerminalError),
}

impl fmt::Display for TerminalStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => formatter.write_str("request is not implemented by this service"),
            Self::NotFound => formatter.write_str("terminal was not found"),
            Self::NotAttached => formatter.write_str("terminal is not attached to session"),
            Self::NotRunning => formatter.write_str("terminal is not running"),
            Self::NotOwner => formatter.write_str("terminal is owned by another session"),
            Self::InvalidSize => formatter.write_str("terminal size must be greater than zero"),
            Self::UnknownProfile => {
                formatter.write_str("terminal profile is not allowed by the Host")
            }
            Self::Backend(error) => error.fmt(formatter),
        }
    }
}

impl Error for TerminalStateError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Backend(error) => Some(error),
            _ => None,
        }
    }
}

impl From<terminal::TerminalError> for TerminalStateError {
    fn from(error: terminal::TerminalError) -> Self {
        Self::Backend(error)
    }
}

pub struct TerminalRegistry {
    backend: Option<Arc<dyn TerminalBackend>>,
    resources: BTreeMap<TerminalId, TerminalResource>,
    /// 操作権を自動解放するまでの無操作時間。
    owner_idle_timeout: Duration,
    /// Terminal を起動したプロセスを Resource Manager へ登録する。
    processes: Option<Arc<dyn process::ProcessRegistry>>,
    workspace_id: Option<editor_core::WorkspaceId>,
}

impl TerminalRegistry {
    pub fn active_in(&self, root: &Path) -> bool {
        self.resources.values().any(|resource| {
            resource.cwd_path.starts_with(root)
                && !matches!(
                    resource.status,
                    TerminalStatus::Exited | TerminalStatus::Terminated | TerminalStatus::Failed
                )
        })
    }

    pub fn new(backend: Option<Arc<dyn TerminalBackend>>) -> Self {
        Self {
            backend,
            resources: BTreeMap::new(),
            owner_idle_timeout: Duration::from_secs(300),
            processes: None,
            workspace_id: None,
        }
    }

    /// 起動した Terminal の PID と子孫を Resource Manager のレジストリへ登録する。
    pub fn with_processes(
        mut self,
        processes: Arc<dyn process::ProcessRegistry>,
        workspace_id: editor_core::WorkspaceId,
    ) -> Self {
        self.processes = Some(processes);
        self.workspace_id = Some(workspace_id);
        self
    }

    fn register_process(
        &self,
        terminal_id: &TerminalId,
        backend_id: &BackendTerminalId,
        backend: &Arc<dyn TerminalBackend>,
    ) {
        let (Some(processes), Some(workspace_id), Some(pid)) = (
            self.processes.as_ref(),
            self.workspace_id.as_ref(),
            backend.process_id(backend_id),
        ) else {
            return;
        };
        let _ = processes.register(process::ProcessRootSpec {
            workspace: Some(workspace_id.clone()),
            kind: process::ProcessKind::Terminal,
            owner: process::ProcessOwner::terminal(terminal_id.as_str()),
            pid,
            label: format!("Terminal {}", terminal_id.as_str()),
        });
    }

    /// 操作権の無操作タイムアウトを変更する（テスト・設定用）。
    pub fn with_owner_idle_timeout(mut self, timeout: Duration) -> Self {
        self.owner_idle_timeout = timeout;
        self
    }

    /// Host が許可する profile 一覧を返す。
    pub fn profiles(&self, cwd: &Path) -> Result<TerminalProfiles, TerminalStateError> {
        let backend = self
            .backend
            .as_ref()
            .ok_or(TerminalStateError::Unsupported)?;
        Ok(backend.profiles(cwd)?)
    }

    pub fn start(
        &mut self,
        terminal_id: TerminalId,
        session_id: &SessionId,
        workspace_id: &WorkspaceId,
        cwd: &Path,
        request: &TerminalStartRequest,
    ) -> Result<(TerminalSnapshot, EventBody), TerminalStateError> {
        let size = request.size;
        require_size(size)?;
        let backend = self
            .backend
            .as_ref()
            .ok_or(TerminalStateError::Unsupported)?;
        let allowed = backend.profiles(cwd)?;
        let profile = allowed
            .resolve(request.profile.as_ref())
            .map_err(|_| TerminalStateError::UnknownProfile)?;
        let backend_id = backend.start_profile(
            cwd,
            size.columns,
            size.rows,
            profile.map(|profile| &profile.id),
        )?;
        self.register_process(&terminal_id, &backend_id, backend);
        let mut resource =
            TerminalResource::new(terminal_id.clone(), backend_id, workspace_id.clone(), size);
        resource.cwd_path = cwd.to_owned();
        resource.cwd = request.cwd.clone();
        resource.attached_sessions.insert(session_id.clone());
        resource.owner = Some(session_id.clone());
        resource.last_activity = Instant::now();
        let snapshot = resource.snapshot(workspace_id);
        self.resources.insert(terminal_id.clone(), resource);
        Ok((
            snapshot,
            EventBody::TerminalStatus {
                terminal_id,
                status: TerminalStatus::Running,
            },
        ))
    }

    pub fn input(
        &mut self,
        session_id: &SessionId,
        workspace_id: &WorkspaceId,
        terminal_id: &TerminalId,
        input: &TerminalInput,
    ) -> Result<TerminalSnapshot, TerminalStateError> {
        let backend = self
            .backend
            .as_ref()
            .ok_or(TerminalStateError::Unsupported)?;
        let resource = require_attached_mut(&mut self.resources, session_id, terminal_id)?;
        resource.require_running()?;
        resource.require_owner(session_id)?;
        resource.last_activity = Instant::now();
        backend.input(&resource.backend_id, input)?;
        Ok(resource.snapshot(workspace_id))
    }

    pub fn resize(
        &mut self,
        session_id: &SessionId,
        workspace_id: &WorkspaceId,
        terminal_id: &TerminalId,
        size: TerminalSize,
    ) -> Result<(TerminalSnapshot, EventBody), TerminalStateError> {
        require_size(size)?;
        let backend = self
            .backend
            .as_ref()
            .ok_or(TerminalStateError::Unsupported)?;
        let resource = require_attached_mut(&mut self.resources, session_id, terminal_id)?;
        resource.require_running()?;
        resource.require_owner(session_id)?;
        resource.last_activity = Instant::now();
        let delta = backend.resize_delta(&resource.backend_id, size.columns, size.rows)?;
        resource.apply_delta(&delta)?;
        let snapshot = resource.snapshot(workspace_id);
        Ok((
            snapshot,
            EventBody::TerminalDelta {
                terminal_id: terminal_id.clone(),
                delta,
            },
        ))
    }

    pub fn terminate(
        &mut self,
        session_id: &SessionId,
        workspace_id: &WorkspaceId,
        terminal_id: &TerminalId,
    ) -> Result<(TerminalSnapshot, Option<EventBody>), TerminalStateError> {
        let backend = self
            .backend
            .as_ref()
            .ok_or(TerminalStateError::Unsupported)?;
        let resource = require_attached_mut(&mut self.resources, session_id, terminal_id)?;
        if resource.status != TerminalStatus::Running {
            return Ok((resource.snapshot(workspace_id), None));
        }
        resource.require_owner(session_id)?;
        backend.terminate(&resource.backend_id)?;
        resource.status = TerminalStatus::Terminated;
        Ok((
            resource.snapshot(workspace_id),
            Some(EventBody::TerminalStatus {
                terminal_id: terminal_id.clone(),
                status: TerminalStatus::Terminated,
            }),
        ))
    }

    /// Terminal backend の変更通知を受け取る。通知は backend の契約どおり 1 度だけ取得できる。
    pub fn take_changes(&self) -> Option<futures_channel::mpsc::UnboundedReceiver<()>> {
        self.backend.as_ref()?.changes()
    }

    pub fn poll(&mut self) -> Result<Vec<EventBody>, TerminalStateError> {
        let Some(backend) = self.backend.as_ref() else {
            return Ok(Vec::new());
        };
        let mut published = Vec::new();
        let idle_timeout = self.owner_idle_timeout;
        for resource in self.resources.values_mut() {
            if resource.owner.is_some() && resource.last_activity.elapsed() >= idle_timeout {
                resource.owner = None;
                published.push(EventBody::TerminalOwner {
                    terminal_id: resource.terminal_id.clone(),
                    owner: None,
                });
            }
            if !matches!(
                resource.status,
                TerminalStatus::Running | TerminalStatus::Disconnected
            ) {
                continue;
            }
            let events = match backend.poll(&resource.backend_id) {
                Ok(events) => events,
                Err(_) => continue,
            };
            for event in events {
                match event {
                    TerminalEvent::Snapshot(snapshot) => {
                        if snapshot.id != resource.backend_id
                            || snapshot.revision < resource.revision
                            || snapshot.screen.size.columns == 0
                            || snapshot.screen.size.rows == 0
                        {
                            continue;
                        }
                        resource.size = snapshot.screen.size;
                        resource.revision = snapshot.revision;
                        resource.screen = snapshot.screen;
                        resource.status = map_backend_status(snapshot.status);
                        published.push(EventBody::SnapshotRequired {
                            workspace_id: resource.workspace_id.clone(),
                            reason: SnapshotReason::EventGap,
                        });
                    }
                    TerminalEvent::Delta(delta) => {
                        if delta.revision <= resource.revision {
                            continue;
                        }
                        if delta.revision != resource.revision.saturating_add(1) {
                            published.push(EventBody::SnapshotRequired {
                                workspace_id: resource.workspace_id.clone(),
                                reason: SnapshotReason::EventGap,
                            });
                            continue;
                        }
                        if resource.apply_delta(&delta).is_err() {
                            // 1 Terminal の delta 失敗で、同じ poll の owner 解放通知まで捨てない。
                            published.push(EventBody::SnapshotRequired {
                                workspace_id: resource.workspace_id.clone(),
                                reason: SnapshotReason::EventGap,
                            });
                            continue;
                        }
                        published.push(EventBody::TerminalDelta {
                            terminal_id: resource.terminal_id.clone(),
                            delta,
                        });
                    }
                    TerminalEvent::Owner { .. } => {}
                    TerminalEvent::Exited { .. } => {
                        resource.status = TerminalStatus::Exited;
                        published.push(EventBody::TerminalStatus {
                            terminal_id: resource.terminal_id.clone(),
                            status: TerminalStatus::Exited,
                        });
                    }
                    TerminalEvent::Failed(_) => {
                        resource.status = TerminalStatus::Failed;
                        published.push(EventBody::TerminalStatus {
                            terminal_id: resource.terminal_id.clone(),
                            status: TerminalStatus::Failed,
                        });
                    }
                }
                if resource.status != TerminalStatus::Running {
                    break;
                }
            }
        }
        Ok(published)
    }

    pub fn snapshots_for_session(
        &mut self,
        session_id: &SessionId,
        workspace_id: &WorkspaceId,
    ) -> Vec<TerminalSnapshot> {
        self.resources
            .values_mut()
            .map(|resource| {
                resource.attached_sessions.insert(session_id.clone());
                resource.snapshot(workspace_id)
            })
            .collect()
    }

    pub fn claim(
        &mut self,
        session_id: &SessionId,
        workspace_id: &WorkspaceId,
        terminal_id: &TerminalId,
    ) -> Result<(TerminalSnapshot, EventBody), TerminalStateError> {
        let resource = require_attached_mut(&mut self.resources, session_id, terminal_id)?;
        resource.require_running()?;
        resource.owner = Some(session_id.clone());
        resource.last_activity = Instant::now();
        Ok((
            resource.snapshot(workspace_id),
            EventBody::TerminalOwner {
                terminal_id: terminal_id.clone(),
                owner: Some(session_id.clone()),
            },
        ))
    }

    pub fn release(
        &mut self,
        session_id: &SessionId,
        workspace_id: &WorkspaceId,
        terminal_id: &TerminalId,
    ) -> Result<(TerminalSnapshot, EventBody), TerminalStateError> {
        let resource = require_attached_mut(&mut self.resources, session_id, terminal_id)?;
        if resource
            .owner
            .as_ref()
            .is_some_and(|owner| owner != session_id)
        {
            return Err(TerminalStateError::NotOwner);
        }
        resource.owner = None;
        Ok((
            resource.snapshot(workspace_id),
            EventBody::TerminalOwner {
                terminal_id: terminal_id.clone(),
                owner: None,
            },
        ))
    }

    pub fn detach(&mut self, session_id: &SessionId) -> Vec<EventBody> {
        let mut events = Vec::new();
        for resource in self.resources.values_mut() {
            resource.attached_sessions.remove(session_id);
            if resource.owner.as_ref() == Some(session_id) {
                resource.owner = None;
                events.push(EventBody::TerminalOwner {
                    terminal_id: resource.terminal_id.clone(),
                    owner: None,
                });
            }
        }
        events
    }
}

impl Drop for TerminalRegistry {
    fn drop(&mut self) {
        let Some(backend) = self.backend.as_ref() else {
            return;
        };
        for resource in self.resources.values() {
            if !matches!(
                resource.status,
                TerminalStatus::Exited | TerminalStatus::Terminated
            ) {
                let _ = backend.terminate(&resource.backend_id);
            }
        }
    }
}

struct TerminalResource {
    terminal_id: TerminalId,
    backend_id: BackendTerminalId,
    workspace_id: WorkspaceId,
    cwd_path: std::path::PathBuf,
    cwd: Option<WorkspaceRelativePath>,
    status: TerminalStatus,
    size: TerminalSize,
    revision: u64,
    screen: TerminalScreen,
    attached_sessions: HashSet<SessionId>,
    /// 入力を許可する Session。`None` は未所有。
    owner: Option<SessionId>,
    /// 最後に操作した時刻。無操作で owner を解放する。
    last_activity: Instant,
}

impl TerminalResource {
    fn new(
        terminal_id: TerminalId,
        backend_id: BackendTerminalId,
        workspace_id: WorkspaceId,
        size: TerminalSize,
    ) -> Self {
        Self {
            terminal_id,
            backend_id,
            workspace_id,
            cwd_path: std::path::PathBuf::new(),
            cwd: None,
            status: TerminalStatus::Running,
            size,
            revision: 0,
            screen: TerminalScreen::empty(size),
            attached_sessions: HashSet::new(),
            owner: None,
            last_activity: Instant::now(),
        }
    }

    fn require_running(&self) -> Result<(), TerminalStateError> {
        if self.status == TerminalStatus::Running {
            Ok(())
        } else {
            Err(TerminalStateError::NotRunning)
        }
    }

    fn apply_delta(&mut self, delta: &TerminalDelta) -> Result<(), TerminalStateError> {
        if delta.revision != self.revision.saturating_add(1) {
            return Err(TerminalStateError::Backend(terminal::TerminalError::new(
                "Terminal revision is not contiguous",
            )));
        }
        self.screen.apply_delta(delta)?;
        self.size = delta.size;
        self.revision = delta.revision;
        Ok(())
    }

    fn snapshot(&self, workspace_id: &WorkspaceId) -> TerminalSnapshot {
        TerminalSnapshot {
            terminal_id: self.terminal_id.clone(),
            workspace_id: workspace_id.clone(),
            cwd: self.cwd.clone(),
            status: self.status,
            revision: self.revision,
            screen: self.screen.clone(),
            owner: self.owner.clone(),
        }
    }

    fn require_owner(&self, session_id: &SessionId) -> Result<(), TerminalStateError> {
        if self.owner.as_ref().is_none_or(|owner| owner == session_id) {
            Ok(())
        } else {
            Err(TerminalStateError::NotOwner)
        }
    }
}

fn require_attached_mut<'a>(
    resources: &'a mut BTreeMap<TerminalId, TerminalResource>,
    session_id: &SessionId,
    terminal_id: &TerminalId,
) -> Result<&'a mut TerminalResource, TerminalStateError> {
    let resource = resources
        .get_mut(terminal_id)
        .ok_or(TerminalStateError::NotFound)?;
    if resource.attached_sessions.contains(session_id) {
        Ok(resource)
    } else {
        Err(TerminalStateError::NotAttached)
    }
}

fn require_size(size: TerminalSize) -> Result<(), TerminalStateError> {
    if size.columns == 0 || size.rows == 0 || size.columns > 512 || size.rows > 256 {
        Err(TerminalStateError::InvalidSize)
    } else {
        Ok(())
    }
}

fn map_backend_status(status: BackendTerminalStatus) -> TerminalStatus {
    match status {
        BackendTerminalStatus::Running => TerminalStatus::Running,
        BackendTerminalStatus::Exited => TerminalStatus::Exited,
        BackendTerminalStatus::Failed => TerminalStatus::Failed,
        BackendTerminalStatus::Disconnected => TerminalStatus::Disconnected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    use terminal::{
        TerminalCell, TerminalError, TerminalEvent, TerminalInput, TerminalKey, TerminalKeyEvent,
        TerminalModifiers, TerminalRow, TerminalRowUpdate, TerminalSnapshot as SourceSnapshot,
        TerminalStatus as SourceStatus,
    };

    #[derive(Default)]
    struct ScriptedSource {
        next: AtomicUsize,
        fail_input: AtomicBool,
        input_calls: AtomicUsize,
        events: Mutex<BTreeMap<String, Result<Vec<TerminalEvent>, TerminalError>>>,
        profiles: Mutex<terminal::TerminalProfiles>,
        started_profiles: Mutex<Vec<Option<String>>>,
    }

    impl TerminalBackend for ScriptedSource {
        fn start(&self, _: &Path, _: u16, _: u16) -> Result<BackendTerminalId, TerminalError> {
            Ok(BackendTerminalId::new(format!(
                "source-{}",
                self.next.fetch_add(1, Ordering::SeqCst) + 1
            )))
        }

        fn start_profile(
            &self,
            cwd: &Path,
            columns: u16,
            rows: u16,
            profile: Option<&terminal::TerminalProfileId>,
        ) -> Result<BackendTerminalId, TerminalError> {
            self.started_profiles
                .lock()
                .unwrap()
                .push(profile.map(|profile| profile.as_str().to_owned()));
            self.start(cwd, columns, rows)
        }

        fn profiles(&self, _: &Path) -> Result<terminal::TerminalProfiles, TerminalError> {
            Ok(self.profiles.lock().unwrap().clone())
        }

        fn input(&self, _: &BackendTerminalId, _: &TerminalInput) -> Result<(), TerminalError> {
            self.input_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_input.load(Ordering::SeqCst) {
                Err(TerminalError::new("terminal input failed"))
            } else {
                Ok(())
            }
        }

        fn resize(&self, _: &BackendTerminalId, _: u16, _: u16) -> Result<(), TerminalError> {
            Ok(())
        }

        fn resize_delta(
            &self,
            _: &BackendTerminalId,
            columns: u16,
            rows: u16,
        ) -> Result<TerminalDelta, TerminalError> {
            Ok(full_delta(1, TerminalSize { columns, rows }, ""))
        }

        fn terminate(&self, _: &BackendTerminalId) -> Result<(), TerminalError> {
            Ok(())
        }

        fn poll(&self, id: &BackendTerminalId) -> Result<Vec<TerminalEvent>, TerminalError> {
            self.events
                .lock()
                .unwrap()
                .remove(id.as_str())
                .unwrap_or_else(|| Ok(Vec::new()))
        }
    }

    fn full_delta(revision: u64, size: TerminalSize, text: &str) -> TerminalDelta {
        let mut first_row = TerminalRow::empty(size.columns);
        if let Some(cell) = first_row.cells.first_mut() {
            *cell = TerminalCell {
                text: text.to_owned(),
                ..TerminalCell::default()
            };
        }
        TerminalDelta {
            revision,
            size,
            changed_rows: (0..size.rows)
                .map(|row| TerminalRowUpdate {
                    row,
                    value: if row == 0 {
                        first_row.clone()
                    } else {
                        TerminalRow::empty(size.columns)
                    },
                })
                .collect(),
            cursor: Default::default(),
            modes: Default::default(),
            scrollback: None,
        }
    }

    fn start_registry(
        backend: Arc<ScriptedSource>,
    ) -> (TerminalRegistry, WorkspaceId, SessionId, TerminalId) {
        let workspace = WorkspaceId::try_new("workspace").unwrap();
        let session = SessionId::try_new("session").unwrap();
        let terminal = TerminalId::try_new("visible").unwrap();
        let mut registry = TerminalRegistry::new(Some(backend));
        registry
            .start(
                terminal.clone(),
                &session,
                &workspace,
                Path::new("."),
                &TerminalStartRequest {
                    workspace_id: workspace.clone(),
                    cwd: None,
                    command: None,
                    profile: None,
                    size: TerminalSize {
                        columns: 80,
                        rows: 24,
                    },
                },
            )
            .unwrap();
        (registry, workspace, session, terminal)
    }

    #[test]
    fn start_rejects_profiles_outside_the_host_catalog() {
        let backend = Arc::new(ScriptedSource::default());
        *backend.profiles.lock().unwrap() = terminal::TerminalProfiles::new(
            vec![
                terminal::TerminalProfile::new("default", "Default", "sh"),
                terminal::TerminalProfile::new("bash", "bash", "bash"),
            ],
            Some(terminal::TerminalProfileId::new("default")),
        );
        let workspace = WorkspaceId::try_new("workspace").unwrap();
        let session = SessionId::try_new("session").unwrap();
        let mut registry = TerminalRegistry::new(Some(backend.clone()));
        let request = |profile: Option<&str>| TerminalStartRequest {
            workspace_id: workspace.clone(),
            cwd: None,
            command: None,
            profile: profile.map(terminal::TerminalProfileId::new),
            size: TerminalSize::new(80, 24),
        };

        let Err(TerminalStateError::UnknownProfile) = registry.start(
            TerminalId::try_new("unknown").unwrap(),
            &session,
            &workspace,
            Path::new("."),
            &request(Some("missing")),
        ) else {
            panic!("unknown profile must be rejected before the backend starts");
        };
        assert!(backend.started_profiles.lock().unwrap().is_empty());

        registry
            .start(
                TerminalId::try_new("allowed").unwrap(),
                &session,
                &workspace,
                Path::new("."),
                &request(Some("bash")),
            )
            .unwrap();
        registry
            .start(
                TerminalId::try_new("default").unwrap(),
                &session,
                &workspace,
                Path::new("."),
                &request(None),
            )
            .unwrap();
        assert_eq!(
            *backend.started_profiles.lock().unwrap(),
            vec![Some("bash".to_owned()), Some("default".to_owned())]
        );
    }

    #[test]
    fn only_the_owner_can_send_input_and_claim_transfers_ownership() {
        let backend = Arc::new(ScriptedSource::default());
        let (mut registry, workspace, owner, terminal) = start_registry(backend.clone());
        let other = SessionId::try_new("session-other").unwrap();
        registry.snapshots_for_session(&other, &workspace);

        assert!(matches!(
            registry.input(
                &other,
                &workspace,
                &terminal,
                &TerminalInput::Text("x".to_owned())
            ),
            Err(TerminalStateError::NotOwner)
        ));
        assert_eq!(backend.input_calls.load(Ordering::SeqCst), 0);

        registry.claim(&other, &workspace, &terminal).unwrap();
        registry
            .input(
                &other,
                &workspace,
                &terminal,
                &TerminalInput::Text("x".to_owned()),
            )
            .unwrap();
        assert_eq!(backend.input_calls.load(Ordering::SeqCst), 1);

        registry.release(&other, &workspace, &terminal).unwrap();
        registry
            .input(
                &owner,
                &workspace,
                &terminal,
                &TerminalInput::Text("y".to_owned()),
            )
            .unwrap();
        assert_eq!(backend.input_calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn idle_owner_is_released_and_notified() {
        let backend = Arc::new(ScriptedSource::default());
        let (mut registry, _workspace, _session, terminal) = start_registry(backend);
        registry.owner_idle_timeout = Duration::ZERO;

        let events = registry.poll().unwrap();

        assert!(events.iter().any(|event| matches!(
            event,
            EventBody::TerminalOwner { terminal_id, owner: None } if terminal_id == &terminal
        )));
    }

    #[test]
    fn owner_release_survives_an_invalid_delta_in_the_same_poll() {
        let backend = Arc::new(ScriptedSource::default());
        let (mut registry, _workspace, _session, terminal) = start_registry(backend.clone());
        registry.owner_idle_timeout = Duration::ZERO;
        backend.events.lock().unwrap().insert(
            "source-1".to_owned(),
            Ok(vec![TerminalEvent::Delta(TerminalDelta {
                revision: 1,
                size: TerminalSize::new(80, 24),
                // 列数と一致しない行は apply_delta が拒否する。
                changed_rows: vec![TerminalRowUpdate {
                    row: 0,
                    value: TerminalRow {
                        cells: Vec::new(),
                        wrapped: false,
                    },
                }],
                cursor: Default::default(),
                modes: Default::default(),
                scrollback: None,
            })]),
        );

        let events = registry.poll().unwrap();

        assert!(events.iter().any(|event| matches!(
            event,
            EventBody::TerminalOwner { terminal_id, owner: None } if terminal_id == &terminal
        )));
    }

    #[test]
    fn snapshot_rebuild_does_not_resend_old_terminal_queries() {
        let backend = Arc::new(ScriptedSource::default());
        let (mut registry, workspace, _session, _terminal) = start_registry(backend.clone());
        backend.events.lock().unwrap().insert(
            "source-1".to_owned(),
            Ok(vec![TerminalEvent::Snapshot(SourceSnapshot {
                id: BackendTerminalId::new("source-1"),
                cwd: ".".into(),
                status: SourceStatus::Running,
                revision: 1,
                screen: TerminalScreen::empty(TerminalSize::new(80, 24)),
            })]),
        );

        assert!(matches!(
            &registry.poll().unwrap()[..],
            [EventBody::SnapshotRequired { workspace_id, reason: SnapshotReason::EventGap }]
                if workspace_id == &workspace
        ));
        assert_eq!(backend.input_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn failed_terminal_reply_does_not_drop_published_events() {
        let backend = Arc::new(ScriptedSource::default());
        let (mut registry, workspace, session, terminal) = start_registry(backend.clone());
        backend.fail_input.store(true, Ordering::SeqCst);
        let input = TerminalInput::Key(TerminalKeyEvent {
            key: TerminalKey::Enter,
            modifiers: TerminalModifiers::default(),
        });
        assert!(
            registry
                .input(&session, &workspace, &terminal, &input)
                .is_err()
        );
        backend.events.lock().unwrap().insert(
            "source-1".to_owned(),
            Ok(vec![TerminalEvent::Delta(full_delta(
                1,
                TerminalSize::new(80, 24),
                "after input",
            ))]),
        );
        let events = registry.poll().unwrap();
        assert!(
            matches!(&events[..], [EventBody::TerminalDelta { delta, .. }] if delta.revision == 1)
        );
    }

    #[test]
    fn reply_failure_snapshot_keeps_the_last_published_screen_and_revision() {
        let backend = Arc::new(ScriptedSource::default());
        let (mut registry, workspace, session, _) = start_registry(backend.clone());
        backend.events.lock().unwrap().insert(
            "source-1".to_owned(),
            Ok(vec![
                TerminalEvent::Delta(full_delta(1, TerminalSize::new(80, 24), "query output")),
                TerminalEvent::Failed("reply write failure".to_owned()),
            ]),
        );
        let published = registry.poll().unwrap();
        assert!(matches!(&published[..], [
            EventBody::TerminalDelta { delta, .. },
            EventBody::TerminalStatus { status: TerminalStatus::Failed, .. }
        ] if delta.revision == 1));
        let snapshots = registry.snapshots_for_session(&session, &workspace);
        assert_eq!(snapshots[0].revision, 1);
        assert_eq!(snapshots[0].status, TerminalStatus::Failed);
        assert!(snapshots[0].screen.plain_text().contains("query output"));
    }

    #[test]
    fn transient_poll_errors_preserve_running_and_provider_snapshots_replace_mapped_state() {
        let backend = Arc::new(ScriptedSource::default());
        let mut registry = TerminalRegistry::new(Some(backend.clone()));
        let workspace = WorkspaceId::try_new("workspace").unwrap();
        let session = SessionId::try_new("session").unwrap();
        let first = TerminalId::try_new("visible-1").unwrap();
        let second = TerminalId::try_new("visible-2").unwrap();
        for id in [&first, &second] {
            registry
                .start(
                    id.clone(),
                    &session,
                    &workspace,
                    Path::new("."),
                    &TerminalStartRequest {
                        workspace_id: workspace.clone(),
                        cwd: None,
                        command: None,
                        profile: None,
                        size: TerminalSize::new(80, 24),
                    },
                )
                .unwrap();
        }
        backend.events.lock().unwrap().insert(
            "source-1".to_owned(),
            Err(TerminalError::new("connection lost")),
        );
        backend.events.lock().unwrap().insert(
            "source-2".to_owned(),
            Ok(vec![TerminalEvent::Delta(full_delta(
                1,
                TerminalSize::new(80, 24),
                "hello",
            ))]),
        );
        assert!(
            matches!(&registry.poll().unwrap()[..], [EventBody::TerminalDelta { terminal_id, delta }] if terminal_id == &second && delta.revision == 1)
        );
        assert_eq!(registry.resources[&first].status, TerminalStatus::Running);

        let snapshot = SourceSnapshot {
            id: BackendTerminalId::new("source-2"),
            cwd: ".".into(),
            status: SourceStatus::Running,
            revision: 7,
            screen: TerminalScreen::empty(TerminalSize::new(132, 40)),
        };
        backend.events.lock().unwrap().insert(
            "source-2".to_owned(),
            Ok(vec![TerminalEvent::Snapshot(snapshot.clone())]),
        );
        assert!(matches!(
            &registry.poll().unwrap()[..],
            [EventBody::SnapshotRequired { workspace_id, reason: SnapshotReason::EventGap }] if workspace_id == &workspace
        ));
        let restored = registry
            .snapshots_for_session(&session, &workspace)
            .into_iter()
            .find(|terminal| terminal.terminal_id == second)
            .unwrap();
        assert_eq!(restored.revision, 7);
        assert_eq!(restored.screen.size, TerminalSize::new(132, 40));

        let mut stale = snapshot;
        stale.revision = 6;
        backend.events.lock().unwrap().insert(
            "source-2".to_owned(),
            Ok(vec![TerminalEvent::Snapshot(stale)]),
        );
        assert!(registry.poll().unwrap().is_empty());

        backend.events.lock().unwrap().insert(
            "source-1".to_owned(),
            Ok(vec![TerminalEvent::Failed(
                "explicit process failure".to_owned(),
            )]),
        );
        let events = registry.poll().unwrap();
        assert!(events.iter().any(|event| matches!(
            event,
            EventBody::TerminalStatus { terminal_id, status: TerminalStatus::Failed }
                if terminal_id == &first
        )));
    }

    #[test]
    fn terminal_deltas_require_contiguous_revisions() {
        let backend = Arc::new(ScriptedSource::default());
        let (mut registry, workspace, _session, _terminal) = start_registry(backend.clone());
        backend.events.lock().unwrap().insert(
            "source-1".to_owned(),
            Ok(vec![TerminalEvent::Delta(full_delta(
                2,
                TerminalSize::new(80, 24),
                "gap",
            ))]),
        );
        assert!(matches!(
            &registry.poll().unwrap()[..],
            [EventBody::SnapshotRequired { workspace_id, reason: SnapshotReason::EventGap }]
                if workspace_id == &workspace
        ));
    }

    #[test]
    fn semantic_input_is_forwarded_without_host_terminal_parsing() {
        let backend = Arc::new(ScriptedSource::default());
        let (mut registry, workspace, session, terminal) = start_registry(backend.clone());
        let input = TerminalInput::Text("日本語".to_owned());
        registry
            .input(&session, &workspace, &terminal, &input)
            .unwrap();
        assert_eq!(backend.input_calls.load(Ordering::SeqCst), 1);
    }
}
