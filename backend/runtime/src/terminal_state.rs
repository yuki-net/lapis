use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
    sync::Arc,
};

use lapis_client_api::{
    EventBody, SessionId, SnapshotReason, TerminalId, TerminalOutputSequence, TerminalSize,
    TerminalSnapshot, TerminalStartRequest, TerminalStatus, WorkspaceId, WorkspaceRelativePath,
};
use lapis_terminal::{TerminalBackend, TerminalEvent, TerminalId as BackendTerminalId};

use crate::BackendStateError;

const MAX_BUFFERED_OUTPUT_BYTES: usize = 256 * 1024;

pub(crate) struct TerminalRegistry {
    backend: Option<Arc<dyn TerminalBackend>>,
    resources: BTreeMap<TerminalId, TerminalResource>,
}

impl TerminalRegistry {
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
        resource
            .screen
            .screen_mut()
            .set_size(size.rows, size.columns);
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
                        resource.output_offset = snapshot.output_offset;
                        resource.buffered_output = snapshot.output;
                        resource.screen = new_screen(snapshot.rows, snapshot.columns);
                        if let Some(screen) = snapshot.screen {
                            resource.screen.process(&screen.data);
                        } else {
                            resource.screen.process(&resource.buffered_output);
                        }
                        resource.append_output(&[]);
                        resource.status = match snapshot.status {
                            lapis_terminal::TerminalStatus::Running => TerminalStatus::Running,
                            lapis_terminal::TerminalStatus::Exited => TerminalStatus::Exited,
                            lapis_terminal::TerminalStatus::Failed => TerminalStatus::Failed,
                            lapis_terminal::TerminalStatus::Disconnected => {
                                TerminalStatus::Disconnected
                            }
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
            let replies = std::mem::take(&mut resource.screen.callbacks_mut().replies);
            if !replies.is_empty() {
                backend.input(&resource.backend_id, &replies)?;
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
    cwd: Option<WorkspaceRelativePath>,
    status: TerminalStatus,
    size: TerminalSize,
    buffered_output: Vec<u8>,
    output_offset: u64,
    screen: vt100::Parser<TerminalReplies>,
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
            terminal_id,
            backend_id,
            workspace_id,
            cwd: None,
            status: TerminalStatus::Running,
            size,
            buffered_output: Vec::new(),
            output_offset: 0,
            screen: new_screen(size.rows, size.columns),
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
        self.screen.process(data);
        self.buffered_output.extend_from_slice(data);
        if self.buffered_output.len() <= MAX_BUFFERED_OUTPUT_BYTES {
            return;
        }
        let start = self.buffered_output.len() - MAX_BUFFERED_OUTPUT_BYTES;
        self.buffered_output.drain(..start);
        self.output_offset = self.output_offset.saturating_add(start as u64);
        self.output_truncated = true;
    }

    fn snapshot(&self, workspace_id: &WorkspaceId) -> TerminalSnapshot {
        let mut screen = if self.screen.screen().alternate_screen() {
            b"\x1b[?1049h".to_vec()
        } else {
            b"\x1b[?1049l".to_vec()
        };
        screen.extend(self.screen.screen().state_formatted());
        TerminalSnapshot {
            terminal_id: self.terminal_id.clone(),
            workspace_id: workspace_id.clone(),
            cwd: self.cwd.clone(),
            status: self.status,
            size: self.size,
            buffered_output: self.buffered_output.clone(),
            output_watermark: self.output_watermark,
            output_truncated: self.output_truncated,
            output_offset: self.output_offset,
            screen: Some(lapis_terminal::TerminalScreenState {
                output_end: self
                    .output_offset
                    .saturating_add(self.buffered_output.len() as u64),
                data: screen,
            }),
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

#[derive(Default)]
struct TerminalReplies {
    replies: Vec<u8>,
}

impl vt100::Callbacks for TerminalReplies {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        i2: Option<u8>,
        params: &[&[u16]],
        command: char,
    ) {
        if self.replies.len() >= 64 * 1024 {
            return;
        }
        let parameter = params
            .first()
            .and_then(|values| values.first())
            .copied()
            .unwrap_or(0);
        let reply = match (i1, i2, parameter, command) {
            (None, None, 6, 'n') => {
                let (row, column) = screen.cursor_position();
                format!("\x1b[{};{}R", row + 1, column + 1)
            }
            (None, None, 5, 'n') => "\x1b[0n".to_owned(),
            (None, None, 0, 'c') => "\x1b[?1;2c".to_owned(),
            (Some(b'>'), None, 0, 'c') => "\x1b[>0;0;0c".to_owned(),
            _ => return,
        };
        self.replies.extend_from_slice(reply.as_bytes());
    }
}

fn new_screen(rows: u16, columns: u16) -> vt100::Parser<TerminalReplies> {
    vt100::Parser::new_with_callbacks(rows, columns, 0, TerminalReplies::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_survives_raw_output_truncation_and_restores_input_modes() {
        let workspace = WorkspaceId::try_new("workspace").unwrap();
        let mut terminal = TerminalResource::new(
            TerminalId::try_new("visible").unwrap(),
            BackendTerminalId::new("source"),
            workspace.clone(),
            TerminalSize {
                columns: 80,
                rows: 24,
            },
        );
        terminal.append_output(&b"old line\r\n".repeat(30_000));
        terminal.append_output(b"\x1b[2J\x1b[Hcurrent screen\x1b[?1h\x1b[?2004h");
        let snapshot = terminal.snapshot(&workspace);
        assert!(snapshot.output_truncated);
        assert!(snapshot.output_offset > 0);
        let state = snapshot.screen.unwrap();
        assert_eq!(
            state.output_end,
            snapshot.output_offset + snapshot.buffered_output.len() as u64
        );
        let mut restored = vt100::Parser::new(24, 80, 0);
        restored.process(&state.data);
        assert_eq!(
            restored.screen().contents(),
            terminal.screen.screen().contents()
        );
        assert!(restored.screen().application_cursor());
        assert!(restored.screen().bracketed_paste());
    }

    #[test]
    fn host_answers_cursor_query_once_even_without_a_client() {
        let mut parser = new_screen(24, 80);
        parser.process(b"\x1b[3;5H\x1b[6");
        assert!(parser.callbacks().replies.is_empty());
        parser.process(b"n");
        assert_eq!(
            std::mem::take(&mut parser.callbacks_mut().replies),
            b"\x1b[3;5R"
        );
        assert!(parser.callbacks().replies.is_empty());
        assert!(
            require_size(TerminalSize {
                columns: u16::MAX,
                rows: u16::MAX
            })
            .is_err()
        );
    }
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
                    &TerminalStartRequest {
                        workspace_id: workspace.clone(),
                        cwd: None,
                        command: None,
                        shell: Default::default(),
                        size: TerminalSize {
                            columns: 80,
                            rows: 24,
                        },
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
            output_offset: 0,
            screen: None,
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
