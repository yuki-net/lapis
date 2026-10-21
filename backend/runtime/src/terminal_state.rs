use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
    sync::Arc,
};

use lapis_client_api::{
    EventBody, SessionId, SnapshotReason, TerminalId, TerminalOutputSequence, TerminalSize,
    TerminalSnapshot, TerminalStatus, WorkspaceId,
};
use lapis_terminal::{TerminalBackend, TerminalEvent, TerminalId as BackendTerminalId};

use crate::BackendStateError;

const MAX_BUFFERED_OUTPUT_BYTES: usize = 256 * 1024;

pub(crate) struct TerminalRegistry {
    backend: Option<Arc<dyn TerminalBackend>>,
    resources: BTreeMap<TerminalId, TerminalResource>,
}

impl TerminalRegistry {
    pub(crate) fn active_in(&self, root: &Path) -> bool {
        self.resources.values().any(|resource| {
            resource.cwd.starts_with(root)
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
        size: TerminalSize,
    ) -> Result<(TerminalSnapshot, EventBody), BackendStateError> {
        require_size(size)?;
        let backend = self
            .backend
            .as_ref()
            .ok_or(BackendStateError::Unsupported)?;
        let backend_id = backend.start(cwd, size.columns, size.rows)?;
        let mut resource =
            TerminalResource::new(terminal_id.clone(), backend_id, workspace_id.clone(), size);
        resource.cwd = cwd.to_owned();
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
        data: &str,
    ) -> Result<TerminalSnapshot, BackendStateError> {
        let backend = self
            .backend
            .as_ref()
            .ok_or(BackendStateError::Unsupported)?;
        let resource = require_attached_mut(&mut self.resources, session_id, terminal_id)?;
        resource.require_running()?;
        backend.input(&resource.backend_id, data.as_bytes())?;
        Ok(resource.snapshot(workspace_id))
    }

    pub(crate) fn resize(
        &mut self,
        session_id: &SessionId,
        workspace_id: &WorkspaceId,
        terminal_id: &TerminalId,
        size: TerminalSize,
    ) -> Result<TerminalSnapshot, BackendStateError> {
        require_size(size)?;
        let backend = self
            .backend
            .as_ref()
            .ok_or(BackendStateError::Unsupported)?;
        let resource = require_attached_mut(&mut self.resources, session_id, terminal_id)?;
        resource.require_running()?;
        backend.resize(&resource.backend_id, size.columns, size.rows)?;
        resource.size = size;
        Ok(resource.snapshot(workspace_id))
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
            if resource.status != TerminalStatus::Running {
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
                            || snapshot.output_sequence
                                < resource
                                    .output_watermark
                                    .map_or(0, |sequence| sequence.value())
                            || snapshot.columns == 0
                            || snapshot.rows == 0
                            || (snapshot.output_sequence == 0 && !snapshot.output.is_empty())
                        {
                            continue;
                        }
                        resource.size = TerminalSize {
                            columns: snapshot.columns,
                            rows: snapshot.rows,
                        };
                        resource.output_watermark = (snapshot.output_sequence != 0)
                            .then_some(TerminalOutputSequence::new(snapshot.output_sequence));
                        resource.output_truncated = snapshot.output_truncated;
                        resource.buffered_output = snapshot.output;
                        resource.append_output(&[]);
                        resource.status = match snapshot.status {
                            lapis_terminal::TerminalStatus::Running => TerminalStatus::Running,
                            lapis_terminal::TerminalStatus::Exited => TerminalStatus::Exited,
                            lapis_terminal::TerminalStatus::Failed => TerminalStatus::Failed,
                        };
                        published.push(EventBody::SnapshotRequired {
                            workspace_id: resource.workspace_id.clone(),
                            reason: SnapshotReason::EventGap,
                        });
                    }
                    TerminalEvent::Output(output) if !output.data.is_empty() => {
                        let sequence = TerminalOutputSequence::new(output.sequence);
                        if resource
                            .output_watermark
                            .is_some_and(|watermark| sequence <= watermark)
                        {
                            continue;
                        }
                        resource.output_watermark = Some(sequence);
                        resource.append_output(&output.data);
                        published.push(EventBody::TerminalOutput {
                            terminal_id: resource.terminal_id.clone(),
                            sequence,
                            data: output.data,
                        });
                    }
                    TerminalEvent::Output(_) => {}
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
    cwd: std::path::PathBuf,
    terminal_id: TerminalId,
    backend_id: BackendTerminalId,
    workspace_id: WorkspaceId,
    status: TerminalStatus,
    size: TerminalSize,
    buffered_output: Vec<u8>,
    output_watermark: Option<TerminalOutputSequence>,
    output_truncated: bool,
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
            cwd: std::path::PathBuf::new(),
            terminal_id,
            backend_id,
            workspace_id,
            status: TerminalStatus::Running,
            size,
            buffered_output: Vec::new(),
            output_watermark: None,
            output_truncated: false,
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

    fn append_output(&mut self, data: &[u8]) {
        self.buffered_output.extend_from_slice(data);
        if self.buffered_output.len() <= MAX_BUFFERED_OUTPUT_BYTES {
            return;
        }
        let start = self.buffered_output.len() - MAX_BUFFERED_OUTPUT_BYTES;
        self.buffered_output.drain(..start);
        self.output_truncated = true;
    }

    fn snapshot(&self, workspace_id: &WorkspaceId) -> TerminalSnapshot {
        TerminalSnapshot {
            terminal_id: self.terminal_id.clone(),
            workspace_id: workspace_id.clone(),
            status: self.status,
            size: self.size,
            buffered_output: self.buffered_output.clone(),
            output_watermark: self.output_watermark,
            output_truncated: self.output_truncated,
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
    if size.columns == 0 || size.rows == 0 {
        Err(BackendStateError::InvalidTerminalSize)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lapis_terminal::{
        TerminalError, TerminalOutput, TerminalSnapshot as SourceSnapshot,
        TerminalStatus as SourceStatus,
    };
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    #[derive(Default)]
    struct ScriptedSource {
        next: AtomicUsize,
        events: Mutex<BTreeMap<String, Result<Vec<TerminalEvent>, TerminalError>>>,
    }

    impl TerminalBackend for ScriptedSource {
        fn start(&self, _: &Path, _: u16, _: u16) -> Result<BackendTerminalId, TerminalError> {
            Ok(BackendTerminalId::new(format!(
                "source-{}",
                self.next.fetch_add(1, Ordering::SeqCst) + 1
            )))
        }

        fn input(&self, _: &BackendTerminalId, _: &[u8]) -> Result<(), TerminalError> {
            Ok(())
        }
        fn resize(&self, _: &BackendTerminalId, _: u16, _: u16) -> Result<(), TerminalError> {
            Ok(())
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
                    TerminalSize {
                        columns: 80,
                        rows: 24,
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
            Ok(vec![TerminalEvent::Output(TerminalOutput {
                sequence: 1,
                data: vec![0xff, 0],
            })]),
        );
        assert!(
            matches!(&registry.poll().unwrap()[..], [EventBody::TerminalOutput { terminal_id, data, .. }] if terminal_id == &second && data == &[0xff, 0])
        );
        assert_eq!(registry.resources[&first].status, TerminalStatus::Running);

        let bytes: Vec<_> = (0..300_000).map(|offset| offset as u8).collect();
        let snapshot = SourceSnapshot {
            id: BackendTerminalId::new("source-2"),
            cwd: ".".into(),
            status: SourceStatus::Running,
            columns: 132,
            rows: 40,
            output: bytes.clone(),
            output_sequence: 7,
            output_truncated: false,
        };
        backend.events.lock().unwrap().insert(
            "source-2".to_owned(),
            Ok(vec![TerminalEvent::Snapshot(snapshot.clone())]),
        );
        assert!(
            matches!(&registry.poll().unwrap()[..], [EventBody::SnapshotRequired { workspace_id, reason: SnapshotReason::EventGap }] if workspace_id == &workspace)
        );
        let restored = registry
            .snapshots_for_session(&session, &workspace)
            .into_iter()
            .find(|terminal| terminal.terminal_id == second)
            .unwrap();
        assert_eq!(
            restored.buffered_output,
            bytes[bytes.len() - MAX_BUFFERED_OUTPUT_BYTES..]
        );
        assert!(restored.output_truncated);
        assert_eq!(restored.output_watermark.unwrap().value(), 7);
        assert_eq!(
            restored.size,
            TerminalSize {
                columns: 132,
                rows: 40
            }
        );
        assert_eq!(restored.status, TerminalStatus::Running);

        let mut mismatched = snapshot.clone();
        mismatched.id = BackendTerminalId::new(second.as_str());
        let mut stale = snapshot.clone();
        stale.output_sequence = 6;
        backend.events.lock().unwrap().insert(
            "source-2".to_owned(),
            Ok(vec![
                TerminalEvent::Snapshot(mismatched),
                TerminalEvent::Snapshot(stale),
                TerminalEvent::Output(TerminalOutput {
                    sequence: 7,
                    data: b"duplicate".to_vec(),
                }),
            ]),
        );
        assert!(registry.poll().unwrap().is_empty());
        assert_eq!(
            registry.resources[&second].buffered_output,
            restored.buffered_output
        );

        backend.events.lock().unwrap().insert(
            "source-1".to_owned(),
            Ok(vec![TerminalEvent::Failed(
                "explicit process failure".to_owned(),
            )]),
        );
        backend.events.lock().unwrap().insert(
            "source-2".to_owned(),
            Ok(vec![TerminalEvent::Output(TerminalOutput {
                sequence: 8,
                data: vec![0xfe, 0],
            })]),
        );
        let events = registry.poll().unwrap();
        assert!(events.iter().any(|event| matches!(event, EventBody::TerminalStatus { terminal_id, status: TerminalStatus::Failed } if terminal_id == &first)));
        assert!(events.iter().any(|event| matches!(event, EventBody::TerminalOutput { terminal_id, sequence, .. } if terminal_id == &second && sequence.value() == 8)));
        assert!(
            registry.resources[&second]
                .buffered_output
                .ends_with(&[0xfe, 0])
        );
        assert!(registry.resources[&second].output_truncated);

        let mut exited = snapshot;
        exited.output_sequence = 9;
        exited.status = SourceStatus::Exited;
        exited.output = vec![0xff, 0, 0x1b];
        exited.output_truncated = true;
        backend.events.lock().unwrap().insert(
            "source-2".to_owned(),
            Ok(vec![TerminalEvent::Snapshot(exited)]),
        );
        assert!(matches!(
            &registry.poll().unwrap()[..],
            [EventBody::SnapshotRequired { .. }]
        ));
        let final_state = registry.resources[&second].snapshot(&workspace);
        assert_eq!(final_state.terminal_id, second);
        assert_eq!(final_state.status, TerminalStatus::Exited);
        assert_eq!(final_state.output_watermark.unwrap().value(), 9);
        assert_eq!(final_state.buffered_output, [0xff, 0, 0x1b]);
        assert!(final_state.output_truncated);
    }
}
