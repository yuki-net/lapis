use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
    sync::Arc,
};

use lapis_client_api::{
    EventBody, SessionId, TerminalId, TerminalOutputSequence, TerminalSize, TerminalSnapshot,
    TerminalStatus, WorkspaceId,
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
        size: TerminalSize,
    ) -> Result<(TerminalSnapshot, EventBody), BackendStateError> {
        require_size(size)?;
        let backend = self
            .backend
            .as_ref()
            .ok_or(BackendStateError::Unsupported)?;
        let backend_id = backend.start(cwd, size.columns, size.rows)?;
        let mut resource = TerminalResource::new(terminal_id.clone(), backend_id, size);
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
                Err(_) => {
                    resource.status = TerminalStatus::Failed;
                    published.push(EventBody::TerminalStatus {
                        terminal_id: resource.terminal_id.clone(),
                        status: TerminalStatus::Failed,
                    });
                    continue;
                }
            };
            for event in events {
                match event {
                    TerminalEvent::Output(data) if !data.is_empty() => {
                        let sequence = resource.next_output_sequence()?;
                        resource.append_output(&data);
                        published.push(EventBody::TerminalOutput {
                            terminal_id: resource.terminal_id.clone(),
                            sequence,
                            data,
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
    status: TerminalStatus,
    size: TerminalSize,
    buffered_output: String,
    output_watermark: Option<TerminalOutputSequence>,
    output_truncated: bool,
    attached_sessions: HashSet<SessionId>,
}

impl TerminalResource {
    fn new(terminal_id: TerminalId, backend_id: BackendTerminalId, size: TerminalSize) -> Self {
        Self {
            terminal_id,
            backend_id,
            status: TerminalStatus::Running,
            size,
            buffered_output: String::new(),
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

    fn next_output_sequence(&mut self) -> Result<TerminalOutputSequence, BackendStateError> {
        let sequence = self
            .output_watermark
            .unwrap_or_default()
            .checked_next()
            .ok_or(BackendStateError::CounterOverflow)?;
        self.output_watermark = Some(sequence);
        Ok(sequence)
    }

    fn append_output(&mut self, data: &str) {
        self.buffered_output.push_str(data);
        if self.buffered_output.len() <= MAX_BUFFERED_OUTPUT_BYTES {
            return;
        }
        let mut start = self.buffered_output.len() - MAX_BUFFERED_OUTPUT_BYTES;
        while !self.buffered_output.is_char_boundary(start) {
            start += 1;
        }
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
