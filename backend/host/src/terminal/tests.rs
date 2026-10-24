use super::*;
use crate::BackendHost;
use lapis_backend_state::{EmbeddedClient, WorkspaceRegistration};
use lapis_client_api::{SessionId, WorkspaceId};
use lapis_platform::LocalWorkspaceRepository;
use lapis_terminal::{
    TerminalBackend, TerminalCell, TerminalDelta, TerminalError, TerminalEvent, TerminalId,
    TerminalInput, TerminalRowUpdate, TerminalScreen, TerminalSize,
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, AtomicUsize, Ordering},
    mpsc,
};
use std::{
    path::Path,
    thread,
    time::{Duration, Instant},
};

#[derive(Default)]
struct ScriptedTerminal {
    events: Mutex<Vec<TerminalEvent>>,
    terminations: AtomicUsize,
    inputs: AtomicUsize,
    revision: AtomicU64,
    size: Mutex<TerminalSize>,
    blocked_input: Mutex<Option<(mpsc::SyncSender<()>, mpsc::Receiver<()>)>>,
}

impl TerminalBackend for ScriptedTerminal {
    fn start(&self, _: &Path, columns: u16, rows: u16) -> Result<TerminalId, TerminalError> {
        *self.size.lock().unwrap() = TerminalSize::new(columns, rows);
        Ok(TerminalId::new("scripted"))
    }

    fn input(&self, _: &TerminalId, _: &TerminalInput) -> Result<(), TerminalError> {
        self.inputs.fetch_add(1, Ordering::SeqCst);
        if let Some((entered, release)) = self.blocked_input.lock().unwrap().take() {
            entered.send(()).unwrap();
            release.recv().unwrap();
        }
        Ok(())
    }

    fn resize(
        &self,
        _: &TerminalId,
        columns: u16,
        rows: u16,
    ) -> Result<TerminalDelta, TerminalError> {
        let size = TerminalSize::new(columns, rows);
        *self.size.lock().unwrap() = size;
        let revision = self.revision.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(delta(revision, size, ""))
    }

    fn poll(&self, _: &TerminalId) -> Result<Vec<TerminalEvent>, TerminalError> {
        let events = std::mem::take(&mut *self.events.lock().unwrap());
        for event in &events {
            if let TerminalEvent::Delta(delta) = event {
                self.revision.fetch_max(delta.revision, Ordering::SeqCst);
            }
        }
        Ok(events)
    }

    fn terminate(&self, _: &TerminalId) -> Result<(), TerminalError> {
        self.terminations.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct Fixture {
    root: tempfile::TempDir,
    host: BackendHost,
    backend: Arc<ScriptedTerminal>,
    terminals: HostTerminals,
    id: TerminalId,
    observer: EmbeddedClient,
    cache: Mutex<TerminalSnapshot>,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let host = BackendHost::start().unwrap();
        let backend = Arc::new(ScriptedTerminal::default());
        let workspace_id = WorkspaceId::try_new("terminal-test").unwrap();
        host.service()
            .register_workspace(
                WorkspaceRegistration::new(
                    workspace_id.clone(),
                    "terminal-test",
                    root.path().canonicalize().unwrap(),
                    Arc::new(LocalWorkspaceRepository),
                )
                .with_terminal(backend.clone()),
            )
            .unwrap();
        host.inner
            .workspaces
            .lock()
            .unwrap()
            .insert(root.path().canonicalize().unwrap(), workspace_id);
        let terminals = HostTerminals::new(host.clone().into());
        let id = terminals.start(root.path(), 80, 24).unwrap();
        let observer = host.connect(root.path()).unwrap();
        let cache = Mutex::new(TerminalSnapshot {
            id: id.clone(),
            cwd: root.path().to_owned(),
            status: ModelTerminalStatus::Running,
            revision: 0,
            screen: TerminalScreen::empty(TerminalSize::new(80, 24)),
        });
        let fixture = Self {
            root,
            host,
            backend,
            terminals,
            id,
            observer,
            cache,
        };
        fixture.snapshot();
        fixture
    }

    fn snapshot(&self) -> lapis_client_api::TerminalSnapshot {
        let ResponseBody::SnapshotResync(response) = self
            .observer
            .request(RequestBody::SnapshotResync(SnapshotRequest {
                workspace_id: self.observer.workspace_id().clone(),
                reason: SnapshotReason::Reconnect,
            }))
            .unwrap()
        else {
            panic!("snapshot rejected")
        };
        response
            .snapshot
            .terminals
            .into_iter()
            .find(|terminal| terminal.terminal_id.as_str() == self.id.as_str())
            .unwrap()
    }

    fn publish(&self, events: Vec<TerminalEvent>, watermark: u64) {
        self.backend.events.lock().unwrap().extend(events);
        until(|| self.snapshot().revision >= watermark);
    }

    fn output(&self, expected: u64) -> String {
        let mut text = String::new();
        until(|| {
            for event in self.terminals.poll(&self.id).unwrap() {
                self.apply(event);
            }
            let cache = self.cache.lock().unwrap();
            text = cache.screen.plain_text();
            cache.revision >= expected && cache.status == ModelTerminalStatus::Running
        });
        text
    }

    fn apply(&self, event: TerminalEvent) {
        let mut cache = self.cache.lock().unwrap();
        match event {
            TerminalEvent::Delta(delta) => {
                assert_eq!(delta.revision, cache.revision + 1);
                cache.screen.apply_delta(&delta).unwrap();
                cache.revision = delta.revision;
            }
            TerminalEvent::Snapshot(snapshot) => {
                assert_eq!(snapshot.id, self.id);
                assert_eq!(snapshot.cwd, self.root.path());
                assert!(snapshot.revision >= cache.revision);
                *cache = snapshot;
            }
            TerminalEvent::Exited { .. } => cache.status = ModelTerminalStatus::Exited,
            TerminalEvent::Failed(_) => cache.status = ModelTerminalStatus::Failed,
        }
    }
}

fn delta(revision: u64, size: TerminalSize, text: &str) -> TerminalDelta {
    let mut row = lapis_terminal::TerminalRow::empty(size.columns);
    if let Some(cell) = row.cells.first_mut() {
        *cell = TerminalCell {
            text: text.to_owned(),
            ..TerminalCell::default()
        };
    }
    TerminalDelta {
        revision,
        size,
        changed_rows: vec![TerminalRowUpdate { row: 0, value: row }],
        cursor: Default::default(),
        modes: Default::default(),
        scrollback: None,
    }
}

fn until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(Instant::now() < deadline, "terminal operation timed out");
        thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn output_gap_resync_preserves_model_and_does_not_replay_consumed_delta() {
    let fixture = Fixture::new();
    fixture.publish(
        vec![TerminalEvent::Delta(delta(
            1,
            TerminalSize::new(80, 24),
            "first",
        ))],
        1,
    );
    assert!(fixture.output(1).contains("first"));
    fixture
        .backend
        .events
        .lock()
        .unwrap()
        .push(TerminalEvent::Delta(delta(
            3,
            TerminalSize::new(80, 24),
            "gap",
        )));
    until(|| {
        fixture.terminals.poll(&fixture.id).unwrap().is_empty() && fixture.snapshot().revision == 1
    });
    fixture.publish(
        vec![
            TerminalEvent::Delta(delta(2, TerminalSize::new(80, 24), "second")),
            TerminalEvent::Delta(delta(3, TerminalSize::new(80, 24), "third")),
        ],
        3,
    );
    assert!(fixture.output(3).contains("third"));
    assert_eq!(fixture.snapshot().status, TerminalStatus::Running);
    assert_eq!(fixture.backend.terminations.load(Ordering::SeqCst), 0);
}

#[test]
fn actual_subscription_overflow_resyncs_revision_and_continues_running() {
    let fixture = Fixture::new();
    fixture.publish(
        (1..=600)
            .map(|revision| TerminalEvent::Delta(delta(revision, TerminalSize::new(80, 24), "row")))
            .collect(),
        600,
    );
    assert!(fixture.output(600).contains("row"));
    assert_eq!(fixture.snapshot().status, TerminalStatus::Running);
    assert_eq!(fixture.backend.terminations.load(Ordering::SeqCst), 0);
}

#[test]
fn reconnect_restores_session_attachment_without_restarting_execution() {
    let fixture = Fixture::new();
    fixture.publish(
        vec![TerminalEvent::Delta(delta(
            1,
            TerminalSize::new(80, 24),
            "before",
        ))],
        1,
    );
    assert!(fixture.output(1).contains("before"));
    fixture
        .host
        .service()
        .disconnect(SessionId::try_new("desktop-1").unwrap())
        .unwrap();
    fixture.publish(
        vec![TerminalEvent::Delta(delta(
            2,
            TerminalSize::new(80, 24),
            "after",
        ))],
        2,
    );
    assert!(fixture.output(2).contains("after"));
    fixture
        .terminals
        .input(&fixture.id, &TerminalInput::Text("input".to_owned()))
        .unwrap();
    assert_eq!(fixture.snapshot().status, TerminalStatus::Running);
    assert_eq!(fixture.backend.terminations.load(Ordering::SeqCst), 0);
}

#[test]
fn exit_releases_attachment_without_conflating_it_with_disconnect() {
    let fixture = Fixture::new();
    fixture.publish(
        vec![TerminalEvent::Delta(delta(
            1,
            TerminalSize::new(80, 24),
            "done",
        ))],
        1,
    );
    assert!(fixture.output(1).contains("done"));
    fixture
        .backend
        .events
        .lock()
        .unwrap()
        .push(TerminalEvent::Exited { code: Some(0) });
    until(|| {
        for event in fixture.terminals.poll(&fixture.id).unwrap() {
            fixture.apply(event);
        }
        fixture.cache.lock().unwrap().status == ModelTerminalStatus::Exited
    });
    assert_eq!(fixture.snapshot().status, TerminalStatus::Exited);
    assert!(fixture.terminals.attachments.lock().unwrap().is_empty());
}

#[test]
fn repeated_resize_and_delta_updates_replace_model_without_duplicates() {
    let fixture = Fixture::new();
    let mut revision = 0;
    for round in 0..4 {
        let columns = 90 + round;
        let rows = 30 + round;
        let resized = fixture
            .terminals
            .resize(&fixture.id, columns, rows)
            .unwrap();
        revision += 1;
        assert_eq!(resized.revision, revision);
        fixture.apply(TerminalEvent::Delta(resized));
        until(|| fixture.snapshot().revision >= revision);
        revision += 1;
        fixture.publish(
            vec![TerminalEvent::Delta(delta(
                revision,
                TerminalSize::new(columns, rows),
                "resized",
            ))],
            revision,
        );
        let text = fixture.output(revision);
        assert!(text.contains("resized"));
        assert_eq!(
            fixture.cache.lock().unwrap().screen.size,
            TerminalSize::new(columns, rows)
        );
    }
    assert_eq!(fixture.snapshot().status, TerminalStatus::Running);
}

#[test]
fn poll_never_waits_for_resync_or_attachment_lock() {
    let fixture = Fixture::new();
    let guard = fixture.terminals.attachments.lock().unwrap();
    let start = Instant::now();
    assert!(fixture.terminals.poll(&fixture.id).unwrap().is_empty());
    assert!(start.elapsed() < Duration::from_millis(100));
    drop(guard);
    let (_sender, receiver) = mpsc::sync_channel(1);
    fixture
        .terminals
        .attachments
        .lock()
        .unwrap()
        .get_mut(fixture.id.as_str())
        .unwrap()
        .recovery = Some(receiver);
    let start = Instant::now();
    assert!(fixture.terminals.poll(&fixture.id).unwrap().is_empty());
    assert!(start.elapsed() < Duration::from_millis(100));
}

#[test]
fn explicit_termination_releases_subscription_without_polling() {
    let fixture = Fixture::new();
    fixture.terminals.terminate(&fixture.id).unwrap();
    assert!(fixture.terminals.attachments.lock().unwrap().is_empty());
    assert_eq!(fixture.snapshot().status, TerminalStatus::Terminated);
    assert_eq!(fixture.backend.terminations.load(Ordering::SeqCst), 1);
    assert!(fixture.root.path().exists());
}

#[test]
fn workspace_event_gap_resyncs_instead_of_appending_an_incomplete_delta() {
    let fixture = Fixture::new();
    fixture.publish(
        vec![TerminalEvent::Delta(delta(
            1,
            TerminalSize::new(80, 24),
            "before",
        ))],
        1,
    );
    assert!(fixture.output(1).contains("before"));
    fixture
        .backend
        .events
        .lock()
        .unwrap()
        .extend([TerminalEvent::Delta(delta(
            3,
            TerminalSize::new(80, 24),
            "gap",
        ))]);
    until(|| {
        fixture.terminals.poll(&fixture.id).unwrap().is_empty() && fixture.snapshot().revision == 1
    });
    fixture.publish(
        vec![TerminalEvent::Delta(delta(
            2,
            TerminalSize::new(80, 24),
            "recovered",
        ))],
        2,
    );
    assert!(fixture.output(2).contains("recovered"));
}

#[test]
fn failed_status_is_not_conflated_with_disconnect() {
    let fixture = Fixture::new();
    fixture
        .backend
        .events
        .lock()
        .unwrap()
        .push(TerminalEvent::Failed("process failed".to_owned()));
    until(|| {
        for event in fixture.terminals.poll(&fixture.id).unwrap() {
            fixture.apply(event);
        }
        fixture.cache.lock().unwrap().status == ModelTerminalStatus::Failed
    });
    assert_eq!(fixture.snapshot().status, TerminalStatus::Failed);
    assert!(fixture.terminals.attachments.lock().unwrap().is_empty());
}

#[test]
fn a_busy_runtime_actor_does_not_block_poll_or_turn_disconnect_into_exit() {
    let fixture = Fixture::new();
    let (entered_sender, entered_receiver) = mpsc::sync_channel(1);
    let (release_sender, release_receiver) = mpsc::sync_channel(1);
    *fixture.backend.blocked_input.lock().unwrap() = Some((entered_sender, release_receiver));
    let client = fixture.observer.clone();
    let terminal_id = lapis_client_api::TerminalId::try_new(fixture.id.as_str()).unwrap();
    let request = thread::spawn(move || {
        client.request(RequestBody::TerminalInput(TerminalInputRequest {
            terminal_id,
            input: TerminalInput::Text("block".to_owned()),
        }))
    });
    entered_receiver
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    let start = Instant::now();
    let first = fixture.terminals.poll(&fixture.id);
    let second = fixture.terminals.poll(&fixture.id);
    let elapsed = start.elapsed();
    release_sender.send(()).unwrap();
    assert!(matches!(
        request.join().unwrap().unwrap(),
        ResponseBody::TerminalInput(_)
    ));
    assert!(first.unwrap().is_empty());
    assert!(second.unwrap().is_empty());
    assert!(elapsed < Duration::from_millis(100));
}

#[test]
fn subscribed_events_at_snapshot_watermark_are_not_replayed() {
    let fixture = Fixture::new();
    fixture.publish(
        vec![TerminalEvent::Delta(delta(
            1,
            TerminalSize::new(80, 24),
            "before",
        ))],
        1,
    );
    assert!(fixture.output(1).contains("before"));
    let client = fixture
        .terminals
        .attachments
        .lock()
        .unwrap()
        .get(fixture.id.as_str())
        .unwrap()
        .client
        .as_ref()
        .unwrap()
        .clone();
    let receiver = client.subscribe().unwrap();
    fixture.publish(
        vec![TerminalEvent::Delta(delta(
            2,
            TerminalSize::new(80, 24),
            "after",
        ))],
        2,
    );
    let ResponseBody::SnapshotResync(response) = client
        .request(RequestBody::SnapshotResync(SnapshotRequest {
            workspace_id: client.workspace_id().clone(),
            reason: SnapshotReason::EventGap,
        }))
        .unwrap()
    else {
        panic!("snapshot rejected")
    };
    let mut attachments = fixture.terminals.attachments.lock().unwrap();
    let attachment = attachments.get_mut(fixture.id.as_str()).unwrap();
    let events = attachment
        .restore(Recovery {
            receiver,
            event_watermark: response.snapshot.event_watermark.value(),
            terminal: response.snapshot.terminals.into_iter().next().unwrap(),
        })
        .unwrap();
    assert!(matches!(&events[..], [TerminalEvent::Snapshot(snapshot)]
        if snapshot.revision == 2 && snapshot.status == ModelTerminalStatus::Running));
    assert!(attachment.poll().unwrap().is_empty());
}

#[test]
fn commands_reattach_after_disconnect_without_waiting_for_ui_poll_or_resync_completion() {
    let fixture = Fixture::new();
    fixture
        .host
        .service()
        .disconnect(SessionId::try_new("desktop-1").unwrap())
        .unwrap();
    fixture.snapshot();
    let (_sender, pending_recovery) = mpsc::sync_channel(1);
    {
        let mut attachments = fixture.terminals.attachments.lock().unwrap();
        let attachment = attachments.get_mut(fixture.id.as_str()).unwrap();
        attachment.receiver = None;
        attachment.recovery = Some(pending_recovery);
    }
    fixture
        .terminals
        .input(&fixture.id, &TerminalInput::Text("exactly once".to_owned()))
        .unwrap();
    assert_eq!(fixture.backend.inputs.load(Ordering::SeqCst), 1);
    let _ = fixture.terminals.resize(&fixture.id, 100, 40).unwrap();
    let snapshot = fixture.snapshot();
    assert_eq!(snapshot.status, TerminalStatus::Running);
    assert_eq!(snapshot.screen.size, TerminalSize::new(100, 40));
    assert_eq!(fixture.backend.terminations.load(Ordering::SeqCst), 0);
    fixture.terminals.terminate(&fixture.id).unwrap();
    assert_eq!(fixture.backend.inputs.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.backend.terminations.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.snapshot().status, TerminalStatus::Terminated);
}
