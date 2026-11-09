use crate::BackendClient;
use crate::client::Client;
use ::terminal::{
    MAX_SCROLLBACK_LINES, TerminalBackend, TerminalError, TerminalEvent, TerminalId, TerminalInput,
    TerminalRevision, TerminalSnapshot, TerminalStatus as ModelTerminalStatus,
};
use backend_state::BackendEventReceiver;
use client_api::{
    EventBody, RequestBody, ResponseBody, SnapshotReason, SnapshotRequest, TerminalInputRequest,
    TerminalResizeRequest, TerminalSize, TerminalStartRequest, TerminalStatus,
    TerminalTerminateRequest, WorkspaceConnectRequest,
};
use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Mutex, TryLockError,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, TryRecvError},
    },
    thread,
    time::{Duration, Instant},
};

pub(super) struct HostTerminals {
    host: BackendClient,
    attachments: Mutex<HashMap<String, Attachment>>,
    changes: futures_channel::mpsc::UnboundedSender<()>,
    changes_rx: Mutex<Option<futures_channel::mpsc::UnboundedReceiver<()>>>,
}

/// 受信スレッドが積み、`poll` が消費するイベントの箱。
#[derive(Default)]
struct Inbox {
    events: Mutex<VecDeque<client_api::EventEnvelope>>,
    closed: AtomicBool,
}

struct Attachment {
    client: Option<Client>,
    inbox: std::sync::Arc<Inbox>,
    changes: futures_channel::mpsc::UnboundedSender<()>,
    terminal_id: client_api::TerminalId,
    cwd: PathBuf,
    event_watermark: Option<u64>,
    revision: TerminalRevision,
    recovery: Option<mpsc::Receiver<Result<Recovery, TerminalError>>>,
    retry_at: Instant,
    finished: bool,
}

struct Recovery {
    receiver: BackendEventReceiver,
    event_watermark: u64,
    terminal: client_api::TerminalSnapshot,
}

impl Attachment {
    fn resync(&mut self, reason: SnapshotReason) -> Result<(), TerminalError> {
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| error("terminal is not attached"))?
            .clone();
        let terminal_id = self.terminal_id.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("lapis-terminal-resync".to_owned())
            .spawn(move || {
                // 通知駆動の UI では再試行タイマーが無いため、接続が戻るまでここで待つ。
                let mut last_error = None;
                for attempt in 0..60 {
                    let result = (|| {
                        match client
                            .request(RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
                                workspace_id: client.workspace_id().clone(),
                            }))
                            .map_err(error)?
                        {
                            ResponseBody::WorkspaceConnect(_) => {}
                            _ => return Err(error("terminal reconnect rejected")),
                        }
                        let receiver = client.subscribe().map_err(error)?;
                        let ResponseBody::SnapshotResync(response) = client
                            .request(RequestBody::SnapshotResync(SnapshotRequest {
                                workspace_id: client.workspace_id().clone(),
                                reason,
                            }))
                            .map_err(error)?
                        else {
                            return Err(error("terminal resync rejected"));
                        };
                        let terminal = response
                            .snapshot
                            .terminals
                            .into_iter()
                            .find(|terminal| terminal.terminal_id == terminal_id)
                            .ok_or_else(|| error("terminal missing from authoritative snapshot"))?;
                        Ok(Recovery {
                            receiver,
                            event_watermark: response.snapshot.event_watermark.value(),
                            terminal,
                        })
                    })();
                    match result {
                        Ok(recovery) => {
                            let _ = sender.send(Ok(recovery));
                            return;
                        }
                        Err(error) => last_error = Some(error),
                    }
                    if attempt < 59 {
                        thread::sleep(Duration::from_millis(500));
                    }
                }
                let _ = sender.send(Err(
                    last_error.unwrap_or_else(|| error("terminal resync stopped"))
                ));
            })
            .map_err(error)?;
        self.inbox.closed.store(true, Ordering::Release);
        self.recovery = Some(receiver);
        Ok(())
    }

    fn status(&mut self, status: TerminalStatus, events: &mut Vec<TerminalEvent>) {
        match status {
            TerminalStatus::Exited | TerminalStatus::Terminated => {
                events.push(TerminalEvent::Exited { code: None });
            }
            TerminalStatus::Failed => {
                events.push(TerminalEvent::Failed("terminal failed".to_owned()));
            }
            _ => return,
        }
        self.detach();
    }

    fn detach(&mut self) {
        self.finished = true;
        let client = self.client.take();
        let _ = thread::Builder::new()
            .name("lapis-terminal-detach".to_owned())
            .spawn(move || drop(client));
    }

    /// 受信したイベントを箱へ移す専用スレッドを開始する。UI は通知で `poll` する。
    fn start_forwarder(&mut self, receiver: BackendEventReceiver) {
        let inbox = std::sync::Arc::new(Inbox::default());
        self.inbox = inbox.clone();
        let changes = self.changes.clone();
        let _ = thread::Builder::new()
            .name("lapis-terminal-events".to_owned())
            .spawn(move || {
                while let Ok(event) = receiver.recv() {
                    if let Ok(mut events) = inbox.events.lock() {
                        events.push_back(event);
                    }
                    if changes.unbounded_send(()).is_err() {
                        return;
                    }
                }
                inbox.closed.store(true, Ordering::Release);
                let _ = changes.unbounded_send(());
            });
    }

    fn restore(&mut self, recovery: Recovery) -> Result<Vec<TerminalEvent>, TerminalError> {
        let terminal = recovery.terminal;
        validate_snapshot(&terminal)?;
        if terminal.terminal_id != self.terminal_id
            || self
                .client
                .as_ref()
                .is_none_or(|client| terminal.workspace_id != *client.workspace_id())
        {
            return Err(error("terminal snapshot identity mismatch"));
        }
        if terminal.revision < self.revision {
            return Err(error("terminal snapshot revision regressed"));
        }
        let status = model_status(terminal.status);
        let snapshot = TerminalSnapshot {
            id: TerminalId::new(self.terminal_id.as_str()),
            cwd: self.cwd.clone(),
            status,
            revision: terminal.revision,
            screen: terminal.screen,
        };
        self.revision = snapshot.revision;
        self.event_watermark = Some(recovery.event_watermark);
        self.start_forwarder(recovery.receiver);
        if matches!(
            status,
            ModelTerminalStatus::Exited | ModelTerminalStatus::Failed
        ) {
            self.detach();
        }
        Ok(vec![TerminalEvent::Snapshot(snapshot)])
    }

    fn poll(&mut self) -> Result<Vec<TerminalEvent>, TerminalError> {
        if self.finished {
            return Ok(Vec::new());
        }
        if let Some(receiver) = &self.recovery {
            let recovered = match receiver.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty) => return Ok(Vec::new()),
                Err(TryRecvError::Disconnected) => Err(error("terminal resync worker stopped")),
            };
            self.recovery = None;
            self.retry_at = Instant::now() + Duration::from_millis(100);
            return self.restore(recovered?);
        }
        if self.inbox.closed.load(Ordering::Acquire) {
            if Instant::now() >= self.retry_at {
                self.resync(SnapshotReason::Reconnect)?;
            }
            return Ok(Vec::new());
        }
        let queued: Vec<_> = {
            let Ok(mut events) = self.inbox.events.lock() else {
                return Ok(Vec::new());
            };
            // VecDeque::drain は途中で捨てると残りも消えるため、pop で必要数だけ取り出す。
            let mut queued = Vec::with_capacity(256);
            while queued.len() < 256 {
                let Some(event) = events.pop_front() else {
                    break;
                };
                queued.push(event);
            }
            queued
        };
        let mut events = Vec::new();
        for event in queued {
            let sequence = event.event_sequence.value();
            if self
                .event_watermark
                .is_some_and(|watermark| sequence <= watermark)
            {
                continue;
            }
            if self
                .event_watermark
                .is_some_and(|watermark| sequence != watermark + 1)
            {
                self.resync(SnapshotReason::EventGap)?;
                break;
            }
            self.event_watermark = Some(sequence);
            match event.body {
                EventBody::TerminalDelta { terminal_id, delta }
                    if terminal_id == self.terminal_id =>
                {
                    if delta.revision <= self.revision {
                        continue;
                    }
                    if delta.revision != self.revision.saturating_add(1) {
                        self.resync(SnapshotReason::EventGap)?;
                        break;
                    }
                    self.revision = delta.revision;
                    events.push(TerminalEvent::Delta(delta));
                }
                EventBody::TerminalStatus {
                    terminal_id,
                    status,
                } if terminal_id == self.terminal_id => {
                    if status == TerminalStatus::Disconnected {
                        self.resync(SnapshotReason::Reconnect)?;
                    } else {
                        self.status(status, &mut events);
                    }
                    if self.finished || self.recovery.is_some() {
                        break;
                    }
                }
                EventBody::SnapshotRequired { reason, .. } => {
                    self.resync(reason)?;
                    break;
                }
                _ => {}
            }
        }
        Ok(events)
    }
}

impl HostTerminals {
    pub(super) fn new(host: BackendClient) -> Self {
        let (changes, changes_rx) = futures_channel::mpsc::unbounded();
        Self {
            host,
            attachments: Mutex::new(HashMap::new()),
            changes,
            changes_rx: Mutex::new(Some(changes_rx)),
        }
    }

    fn command(
        &self,
        id: &TerminalId,
        body: impl FnOnce(&client_api::TerminalId) -> RequestBody,
    ) -> Result<ResponseBody, TerminalError> {
        let attachments = self.attachments.lock().map_err(error)?;
        let attachment = attachments
            .get(id.as_str())
            .ok_or_else(|| TerminalError::new("terminal is not attached"))?;
        let client = attachment
            .client
            .as_ref()
            .ok_or_else(|| error("terminal is no longer attached"))?
            .clone();
        let request = body(&attachment.terminal_id);
        drop(attachments);
        let mut response = client.request(request.clone()).map_err(error)?;
        if matches!(&response, ResponseBody::Error(problem)
            if matches!(problem.code.as_str(), client_api::FORBIDDEN | client_api::INVALID_REQUEST))
        {
            match client
                .request(RequestBody::WorkspaceConnect(WorkspaceConnectRequest {
                    workspace_id: client.workspace_id().clone(),
                }))
                .map_err(error)?
            {
                ResponseBody::WorkspaceConnect(_) => {}
                _ => return Err(error("terminal reconnect rejected")),
            }
            let ResponseBody::SnapshotResync(snapshot) = client
                .request(RequestBody::SnapshotResync(SnapshotRequest {
                    workspace_id: client.workspace_id().clone(),
                    reason: SnapshotReason::Reconnect,
                }))
                .map_err(error)?
            else {
                return Err(error("terminal reattachment rejected"));
            };
            if !snapshot
                .snapshot
                .terminals
                .iter()
                .any(|terminal| terminal.terminal_id.as_str() == id.as_str())
            {
                return Err(error("terminal missing from authoritative snapshot"));
            }
            response = client.request(request).map_err(error)?;
        }
        match response {
            response @ (ResponseBody::TerminalInput(_)
            | ResponseBody::TerminalResize(_)
            | ResponseBody::TerminalTerminate(_)
            | ResponseBody::TerminalClaim(_)
            | ResponseBody::TerminalRelease(_)) => Ok(response),
            ResponseBody::Error(problem) => Err(error(
                problem.detail.as_deref().unwrap_or(problem.code.as_str()),
            )),
            _ => Err(error("unexpected terminal response")),
        }
    }
}

impl TerminalBackend for HostTerminals {
    fn start(&self, cwd: &Path, columns: u16, rows: u16) -> Result<TerminalId, TerminalError> {
        self.start_with_shell(cwd, columns, rows, terminal::TerminalShell::Default)
    }

    fn start_with_shell(
        &self,
        cwd: &Path,
        columns: u16,
        rows: u16,
        shell: terminal::TerminalShell,
    ) -> Result<TerminalId, TerminalError> {
        let (client, relative_cwd) = self.host.connect_directory(cwd).map_err(error)?;
        let receiver = client.subscribe().map_err(error)?;
        let response = client
            .request(RequestBody::TerminalStart(TerminalStartRequest {
                workspace_id: client.workspace_id().clone(),
                cwd: relative_cwd,
                command: None,
                shell,
                size: TerminalSize { columns, rows },
            }))
            .map_err(error)?;
        let ResponseBody::TerminalStart(response) = response else {
            return Err(error("terminal start rejected"));
        };
        let initial_revision = response.terminal.revision;
        let terminal_id = response.terminal.terminal_id;
        let id = TerminalId::new(terminal_id.as_str());
        let mut attachment = Attachment {
            client: Some(client),
            inbox: std::sync::Arc::new(Inbox::default()),
            changes: self.changes.clone(),
            terminal_id,
            cwd: cwd.to_owned(),
            event_watermark: None,
            revision: initial_revision,
            recovery: None,
            retry_at: Instant::now(),
            finished: false,
        };
        attachment.start_forwarder(receiver);
        self.attachments
            .lock()
            .map_err(error)?
            .insert(id.as_str().to_owned(), attachment);
        Ok(id)
    }

    fn list(&self, cwd: &Path) -> Result<Vec<TerminalSnapshot>, TerminalError> {
        let (client, _) = self.host.connect_directory(cwd).map_err(error)?;
        let root = self.host.root_for(client.workspace_id()).map_err(error)?;
        let ResponseBody::SnapshotResync(response) = client
            .request(RequestBody::SnapshotResync(SnapshotRequest {
                workspace_id: client.workspace_id().clone(),
                reason: SnapshotReason::Reconnect,
            }))
            .map_err(error)?
        else {
            return Err(error("terminal list rejected"));
        };
        response
            .snapshot
            .terminals
            .into_iter()
            .map(|terminal| model_snapshot(&root, terminal))
            .collect::<Result<Vec<_>, _>>()
    }

    fn attach(&self, cwd: &Path, id: &TerminalId) -> Result<TerminalSnapshot, TerminalError> {
        let (client, _) = self.host.connect_directory(cwd).map_err(error)?;
        let root = self.host.root_for(client.workspace_id()).map_err(error)?;
        let receiver = client.subscribe().map_err(error)?;
        let ResponseBody::SnapshotResync(response) = client
            .request(RequestBody::SnapshotResync(SnapshotRequest {
                workspace_id: client.workspace_id().clone(),
                reason: SnapshotReason::Reconnect,
            }))
            .map_err(error)?
        else {
            return Err(error("terminal attach rejected"));
        };
        let terminal = response
            .snapshot
            .terminals
            .into_iter()
            .find(|terminal| terminal.terminal_id.as_str() == id.as_str())
            .ok_or_else(|| error("terminal is not present on the Host"))?;
        let snapshot = model_snapshot(&root, terminal.clone())?;
        let mut attachment = Attachment {
            client: Some(client),
            inbox: std::sync::Arc::new(Inbox::default()),
            changes: self.changes.clone(),
            terminal_id: terminal.terminal_id.clone(),
            cwd: snapshot.cwd.clone(),
            event_watermark: None,
            revision: terminal.revision,
            recovery: None,
            retry_at: Instant::now(),
            finished: false,
        };
        attachment.restore(Recovery {
            receiver,
            event_watermark: response.snapshot.event_watermark.value(),
            terminal,
        })?;
        if !attachment.finished {
            self.attachments
                .lock()
                .map_err(error)?
                .insert(id.as_str().to_owned(), attachment);
        }
        Ok(snapshot)
    }

    fn input(&self, id: &TerminalId, input: &TerminalInput) -> Result<(), TerminalError> {
        let input = input.clone();
        self.command(id, |terminal_id| {
            RequestBody::TerminalInput(TerminalInputRequest {
                terminal_id: terminal_id.clone(),
                input,
            })
        })
        .map(|_| ())
    }

    fn resize(&self, id: &TerminalId, columns: u16, rows: u16) -> Result<(), TerminalError> {
        self.command(id, |terminal_id| {
            RequestBody::TerminalResize(TerminalResizeRequest {
                terminal_id: terminal_id.clone(),
                size: TerminalSize { columns, rows },
            })
        })?;
        // 画面の更新は EventBody::TerminalDelta で届く。応答には状態を載せない。
        Ok(())
    }

    fn poll(&self, id: &TerminalId) -> Result<Vec<TerminalEvent>, TerminalError> {
        let mut attachments = match self.attachments.try_lock() {
            Ok(attachments) => attachments,
            Err(TryLockError::WouldBlock) => return Ok(Vec::new()),
            Err(problem) => return Err(error(problem)),
        };
        let attachment = attachments
            .get_mut(id.as_str())
            .ok_or_else(|| error("terminal is not attached"))?;
        let events = attachment.poll()?;
        if attachment.finished {
            attachments.remove(id.as_str());
        }
        Ok(events)
    }

    fn terminate(&self, id: &TerminalId) -> Result<(), TerminalError> {
        self.command(id, |terminal_id| {
            RequestBody::TerminalTerminate(TerminalTerminateRequest {
                terminal_id: terminal_id.clone(),
            })
        })
        .map(|_| ())?;
        self.attachments.lock().map_err(error)?.remove(id.as_str());
        Ok(())
    }

    fn claim(&self, id: &TerminalId) -> Result<(), TerminalError> {
        self.command(id, |terminal_id| {
            RequestBody::TerminalClaim(client_api::TerminalClaimRequest {
                terminal_id: terminal_id.clone(),
            })
        })
        .map(|_| ())
    }

    fn release(&self, id: &TerminalId) -> Result<(), TerminalError> {
        self.command(id, |terminal_id| {
            RequestBody::TerminalRelease(client_api::TerminalReleaseRequest {
                terminal_id: terminal_id.clone(),
            })
        })
        .map(|_| ())
    }

    fn changes(&self) -> Option<futures_channel::mpsc::UnboundedReceiver<()>> {
        self.changes_rx.lock().ok()?.take()
    }
}

fn model_status(status: TerminalStatus) -> ModelTerminalStatus {
    match status {
        TerminalStatus::Exited | TerminalStatus::Terminated => ModelTerminalStatus::Exited,
        TerminalStatus::Failed => ModelTerminalStatus::Failed,
        TerminalStatus::Disconnected => ModelTerminalStatus::Disconnected,
        TerminalStatus::Starting | TerminalStatus::Running => ModelTerminalStatus::Running,
    }
}

fn model_snapshot(
    root: &Path,
    terminal: client_api::TerminalSnapshot,
) -> Result<TerminalSnapshot, TerminalError> {
    validate_snapshot(&terminal)?;
    Ok(TerminalSnapshot {
        id: TerminalId::new(terminal.terminal_id.as_str()),
        cwd: terminal
            .cwd
            .as_ref()
            .map_or_else(|| root.to_owned(), |cwd| root.join(cwd.as_str())),
        status: model_status(terminal.status),
        revision: terminal.revision,
        screen: terminal.screen,
    })
}

fn validate_snapshot(terminal: &client_api::TerminalSnapshot) -> Result<(), TerminalError> {
    let screen = &terminal.screen;
    if screen.size.columns == 0
        || screen.size.columns > 512
        || screen.size.rows == 0
        || screen.size.rows > 256
        || screen.rows.len() != screen.size.rows as usize
        || screen
            .rows
            .iter()
            .any(|row| row.cells.len() != screen.size.columns as usize)
        || screen.scrollback.len() > MAX_SCROLLBACK_LINES
        || screen
            .scrollback
            .iter()
            .any(|row| row.cells.len() != screen.size.columns as usize)
    {
        return Err(error("terminal snapshot is invalid"));
    }
    Ok(())
}

fn error(value: impl std::fmt::Display) -> TerminalError {
    TerminalError::new(value.to_string())
}

#[cfg(test)]
mod tests;
