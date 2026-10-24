use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
    sync::Arc,
};

use lapis_client_api::{
    EventBody, SessionId, SnapshotReason, TerminalId, TerminalInput, TerminalSize,
    TerminalSnapshot, TerminalStartRequest, TerminalStatus, WorkspaceId, WorkspaceRelativePath,
};
use lapis_terminal::{
    TerminalBackend, TerminalDelta, TerminalEvent, TerminalId as BackendTerminalId, TerminalScreen,
    TerminalStatus as BackendTerminalStatus,
};

use crate::BackendStateError;

pub(crate) struct TerminalRegistry {
    backend: Option<Arc<dyn TerminalBackend>>,
    resources: BTreeMap<TerminalId, TerminalResource>,
}

impl TerminalRegistry {
    pub(crate) fn active_in(&self, root: &Path) -> bool {
        self.resources.values().any(|resource| {
            resource.cwd_path.starts_with(root)
                && !matches!(
                    resource.status,
                    TerminalStatus::Exited | TerminalStatus::Terminated | TerminalStatus::Failed
                )
        })
    }

    pub(crate) fn new(backend: Option<Arc<dyn TerminalBackend>>) -> Self {
        Self {
            backend,
            resources: BTreeMap::new(),
        }
    }

    pub(crate) fn start(
        &mut self,
        terminal_id: TerminalId,
        session_id: &SessionId,
        workspace_id: &WorkspaceId,
        cwd: &Path,
        request: &TerminalStartRequest,
    ) -> Result<(TerminalSnapshot, EventBody), BackendStateError> {
        let size = request.size;
        require_size(size)?;
        let backend = self
            .backend
            .as_ref()
            .ok_or(BackendStateError::Unsupported)?;
        let backend_id = backend.start_with_shell(cwd, size.columns, size.rows, request.shell)?;
        let mut resource =
            TerminalResource::new(terminal_id.clone(), backend_id, workspace_id.clone(), size);
        resource.cwd_path = cwd.to_owned();
        resource.cwd = request.cwd.clone();
        resource.attached_sessions.insert(session_id.clone());
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

    pub(crate) fn input(
        &mut self,
        session_id: &SessionId,
        workspace_id: &WorkspaceId,
        terminal_id: &TerminalId,
        input: &TerminalInput,
    ) -> Result<TerminalSnapshot, BackendStateError> {
        let backend = self
            .backend
            .as_ref()
            .ok_or(BackendStateError::Unsupported)?;
        let resource = require_attached_mut(&mut self.resources, session_id, terminal_id)?;
        resource.require_running()?;
        backend.input(&resource.backend_id, input)?;
        Ok(resource.snapshot(workspace_id))
    }

    pub(crate) fn resize(
        &mut self,
        session_id: &SessionId,
        workspace_id: &WorkspaceId,
        terminal_id: &TerminalId,
        size: TerminalSize,
    ) -> Result<(TerminalSnapshot, EventBody), BackendStateError> {
        require_size(size)?;
        let backend = self
            .backend
            .as_ref()
            .ok_or(BackendStateError::Unsupported)?;
        let resource = require_attached_mut(&mut self.resources, session_id, terminal_id)?;
        resource.require_running()?;
        let delta = backend.resize(&resource.backend_id, size.columns, size.rows)?;
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

    pub(crate) fn terminate(
        &mut self,
        session_id: &SessionId,
        workspace_id: &WorkspaceId,
        terminal_id: &TerminalId,
    ) -> Result<(TerminalSnapshot, Option<EventBody>), BackendStateError> {
        let backend = self
            .backend
            .as_ref()
            .ok_or(BackendStateError::Unsupported)?;
        let resource = require_attached_mut(&mut self.resources, session_id, terminal_id)?;
        if resource.status != TerminalStatus::Running {
            return Ok((resource.snapshot(workspace_id), None));
        }
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

    pub(crate) fn poll(&mut self) -> Result<Vec<EventBody>, BackendStateError> {
        let Some(backend) = self.backend.as_ref() else {
            return Ok(Vec::new());
        };
        let mut published = Vec::new();
        for resource in self.resources.values_mut() {
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
                        resource.apply_delta(&delta)?;
                        published.push(EventBody::TerminalDelta {
                            terminal_id: resource.terminal_id.clone(),
                            delta,
                        });
                    }
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

    pub(crate) fn snapshots_for_session(
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

    pub(crate) fn detach(&mut self, session_id: &SessionId) {
        for resource in self.resources.values_mut() {
            resource.attached_sessions.remove(session_id);
        }
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
        }
    }

    fn require_running(&self) -> Result<(), BackendStateError> {
        if self.status == TerminalStatus::Running {
            Ok(())
        } else {
            Err(BackendStateError::TerminalNotRunning)
        }
    }

    fn apply_delta(&mut self, delta: &TerminalDelta) -> Result<(), BackendStateError> {
        if delta.revision != self.revision.saturating_add(1) {
            return Err(BackendStateError::Terminal(
                lapis_terminal::TerminalError::new("Terminal revision is not contiguous"),
            ));
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
        }
    }
}

fn require_attached_mut<'a>(
    resources: &'a mut BTreeMap<TerminalId, TerminalResource>,
    session_id: &SessionId,
    terminal_id: &TerminalId,
) -> Result<&'a mut TerminalResource, BackendStateError> {
    let resource = resources
        .get_mut(terminal_id)
        .ok_or(BackendStateError::TerminalNotFound)?;
    if resource.attached_sessions.contains(session_id) {
        Ok(resource)
    } else {
        Err(BackendStateError::TerminalNotAttached)
    }
}

fn require_size(size: TerminalSize) -> Result<(), BackendStateError> {
    if size.columns == 0 || size.rows == 0 || size.columns > 512 || size.rows > 256 {
        Err(BackendStateError::InvalidTerminalSize)
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
    use lapis_terminal::{
        TerminalCell, TerminalError, TerminalEvent, TerminalInput, TerminalKey, TerminalKeyEvent,
        TerminalModifiers, TerminalRow, TerminalRowUpdate, TerminalShell,
        TerminalSnapshot as SourceSnapshot, TerminalStatus as SourceStatus,
    };
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    #[derive(Default)]
    struct ScriptedSource {
        next: AtomicUsize,
        fail_input: AtomicBool,
        input_calls: AtomicUsize,
        events: Mutex<BTreeMap<String, Result<Vec<TerminalEvent>, TerminalError>>>,
    }

    impl TerminalBackend for ScriptedSource {
        fn start(&self, _: &Path, _: u16, _: u16) -> Result<BackendTerminalId, TerminalError> {
            Ok(BackendTerminalId::new(format!(
                "source-{}",
                self.next.fetch_add(1, Ordering::SeqCst) + 1
            )))
        }

        fn input(&self, _: &BackendTerminalId, _: &TerminalInput) -> Result<(), TerminalError> {
            self.input_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_input.load(Ordering::SeqCst) {
                Err(TerminalError::new("terminal input failed"))
            } else {
                Ok(())
            }
        }

        fn resize(
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
                    shell: TerminalShell::Default,
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
                        shell: TerminalShell::Default,
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
