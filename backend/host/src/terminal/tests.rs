use super::*;
use lapis_backend_state::WorkspaceRegistration;
use lapis_client_api::{SessionId, WorkspaceId};
use lapis_platform::LocalWorkspaceRepository;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Default)]
struct ScriptedTerminal {
    events: Mutex<Vec<TerminalEvent>>,
    terminations: AtomicUsize,
    inputs: AtomicUsize,
    blocked_input: Mutex<Option<(mpsc::SyncSender<()>, mpsc::Receiver<()>)>>,
}

impl TerminalBackend for ScriptedTerminal {
    fn start(&self, _: &Path, _: u16, _: u16) -> Result<TerminalId, TerminalError> {
        Ok(TerminalId::new("scripted"))
    }

    fn input(&self, _: &TerminalId, _: &[u8]) -> Result<(), TerminalError> {
        self.inputs.fetch_add(1, Ordering::SeqCst);
        if let Some((entered, release)) = self.blocked_input.lock().unwrap().take() {
            entered.send(()).unwrap();
            release.recv().unwrap();
        }
        Ok(())
    }

    fn resize(&self, _: &TerminalId, _: u16, _: u16) -> Result<(), TerminalError> {
        Ok(())
    }

    fn poll(&self, _: &TerminalId) -> Result<Vec<TerminalEvent>, TerminalError> {
        Ok(std::mem::take(&mut *self.events.lock().unwrap()))
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
            columns: 80,
            rows: 24,
            output: Vec::new(),
            output_sequence: 0,
            output_truncated: false,
            output_offset: 0,
            screen: None,
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
        until(|| {
            self.snapshot()
                .output_watermark
                .is_some_and(|sequence| sequence.value() == watermark)
        });
    }

    fn output(&self, expected: u64) -> Vec<u8> {
        let previous_length = self.cache.lock().unwrap().output.len();
        until(|| {
            for event in self.terminals.poll(&self.id).unwrap() {
                self.apply(event);
            }
            let cache = self.cache.lock().unwrap();
            assert_eq!(cache.status, ModelTerminalStatus::Running);
            cache.output_sequence == expected
        });
        self.cache.lock().unwrap().output[previous_length..].to_vec()
    }

    fn apply(&self, event: TerminalEvent) {
        let mut cache = self.cache.lock().unwrap();
        match event {
            TerminalEvent::Output(output) => {
                assert!(output.sequence > cache.output_sequence);
                cache.output_sequence = output.sequence;
                cache.output.extend(output.data);
                if cache.output.len() > 256 * 1024 {
                    let excess = cache.output.len() - 256 * 1024;
                    cache.output.drain(..excess);
                    cache.output_truncated = true;
                }
            }
            TerminalEvent::Snapshot(snapshot) => {
                assert_eq!(snapshot.id, self.id);
                assert_eq!(snapshot.cwd, self.root.path());
                assert!(snapshot.output_sequence >= cache.output_sequence);
                assert!(snapshot.output.len() <= 256 * 1024);
                *cache = snapshot;
            }
            TerminalEvent::Exited { .. } => cache.status = ModelTerminalStatus::Exited,
            TerminalEvent::Failed(_) => cache.status = ModelTerminalStatus::Failed,
        }
    }
}

fn until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(Instant::now() < deadline, "terminal operation timed out");
        thread::sleep(Duration::from_millis(1));
    }
}

fn output(sequence: u64, data: &[u8]) -> TerminalEvent {
    TerminalEvent::Output(TerminalOutput {
        sequence,
        data: data.to_vec(),
    })
}

#[test]
fn output_gap_resync_preserves_binary_and_does_not_replay_consumed_bytes() {
    let fixture = Fixture::new();
    fixture.publish(vec![output(1, &[0xff, 0, 0x1b])], 1);
    assert_eq!(fixture.output(1), [0xff, 0, 0x1b]);
    fixture.publish(vec![output(3, &[0x80, b'x'])], 3);
    assert_eq!(fixture.output(3), [0x80, b'x']);
    fixture.publish(vec![output(4, b"next")], 4);
    assert_eq!(fixture.output(4), b"next");
    assert!(fixture.terminals.poll(&fixture.id).unwrap().is_empty());
    assert_eq!(fixture.snapshot().status, TerminalStatus::Running);
    assert_eq!(fixture.backend.terminations.load(Ordering::SeqCst), 0);
}

#[test]
fn actual_subscription_overflow_resyncs_output_and_continues_running() {
    let fixture = Fixture::new();
    fixture.publish(
        (1..=600)
            .map(|sequence| output(sequence, &[sequence as u8]))
            .collect(),
        600,
    );
    assert_eq!(
        fixture.output(600),
        (1..=600).map(|sequence| sequence as u8).collect::<Vec<_>>()
    );
    fixture.publish(vec![output(601, &[0, 0xfe])], 601);
    assert_eq!(fixture.output(601), [0, 0xfe]);
    assert_eq!(fixture.snapshot().status, TerminalStatus::Running);
    assert_eq!(fixture.backend.terminations.load(Ordering::SeqCst), 0);
}

#[test]
fn reconnect_restores_session_attachment_without_restarting_execution() {
    let fixture = Fixture::new();
    fixture.publish(vec![output(1, b"before")], 1);
    assert_eq!(fixture.output(1), b"before");
    fixture
        .host
        .service()
        .disconnect(SessionId::try_new("desktop-1").unwrap())
        .unwrap();
    fixture.publish(vec![output(2, b"after")], 2);
    assert_eq!(fixture.output(2), b"after");
    fixture.terminals.input(&fixture.id, b"input").unwrap();
    assert_eq!(fixture.snapshot().status, TerminalStatus::Running);
    assert_eq!(fixture.backend.terminations.load(Ordering::SeqCst), 0);
}

#[test]
fn overflow_recovers_authoritative_exit_and_releases_attachment() {
    let fixture = Fixture::new();
    let mut events: Vec<_> = (1..=600)
        .map(|sequence| output(sequence, &[0xff]))
        .collect();
    events.push(TerminalEvent::Exited { code: Some(0) });
    fixture.publish(events, 600);
    until(|| {
        for event in fixture.terminals.poll(&fixture.id).unwrap() {
            fixture.apply(event);
        }
        fixture.cache.lock().unwrap().status == ModelTerminalStatus::Exited
    });
    assert_eq!(fixture.cache.lock().unwrap().output, vec![0xff; 600]);
    assert_eq!(fixture.snapshot().status, TerminalStatus::Exited);
    assert!(fixture.terminals.attachments.lock().unwrap().is_empty());
    assert_eq!(fixture.backend.terminations.load(Ordering::SeqCst), 0);
}

#[test]
fn repeated_truncated_overflows_replace_cache_without_duplicates_or_false_exit() {
    let fixture = Fixture::new();
    let mut sequence = 0;
    for round in 0..4 {
        let resized = fixture
            .observer
            .request(RequestBody::TerminalResize(TerminalResizeRequest {
                terminal_id: lapis_client_api::TerminalId::try_new(fixture.id.as_str()).unwrap(),
                size: TerminalSize {
                    columns: 90 + round,
                    rows: 30 + round,
                },
            }))
            .unwrap();
        assert!(matches!(resized, ResponseBody::TerminalResize(_)));
        let mut events = Vec::new();
        for _ in 0..600 {
            sequence += 1;
            let data: Vec<_> = (0..600).map(|offset| (sequence + offset) as u8).collect();
            events.push(output(sequence, &data));
        }
        if round == 3 {
            events.push(TerminalEvent::Exited { code: Some(0) });
        }
        fixture.publish(events, sequence);
        let authoritative = fixture.snapshot();
        assert!(authoritative.output_truncated);
        let expected_status = if round == 3 {
            ModelTerminalStatus::Exited
        } else {
            ModelTerminalStatus::Running
        };
        let mut replacements = 0;
        until(|| {
            for event in fixture.terminals.poll(&fixture.id).unwrap() {
                if matches!(event, TerminalEvent::Snapshot(_)) {
                    replacements += 1;
                }
                fixture.apply(event);
                let status = fixture.cache.lock().unwrap().status;
                if round != 3 {
                    assert_eq!(status, ModelTerminalStatus::Running);
                } else {
                    assert_ne!(status, ModelTerminalStatus::Failed);
                }
            }
            let cache = fixture.cache.lock().unwrap();
            cache.output_sequence == sequence && cache.status == expected_status
        });
        assert_eq!(replacements, 1);
        let cache = fixture.cache.lock().unwrap();
        assert_eq!(cache.output, authoritative.buffered_output);
        assert!(cache.output_truncated);
        assert_eq!(cache.columns, 90 + round);
        assert_eq!(cache.rows, 30 + round);
        assert_eq!(
            cache.output_sequence,
            authoritative.output_watermark.unwrap().value()
        );
        drop(cache);
        if round != 3 {
            assert!(fixture.terminals.poll(&fixture.id).unwrap().is_empty());
            fixture
                .terminals
                .input(&fixture.id, b"still attached")
                .unwrap();
        }
    }
    assert!(fixture.terminals.attachments.lock().unwrap().is_empty());
    assert_eq!(fixture.backend.terminations.load(Ordering::SeqCst), 0);
}

#[test]
fn poll_never_waits_for_resync_or_attachment_lock() {
    let fixture = Fixture::new();
    let guard = fixture.terminals.attachments.lock().unwrap();
    let start = Instant::now();
    assert!(fixture.terminals.poll(&fixture.id).unwrap().is_empty());
    assert!(start.elapsed() < Duration::from_millis(100));
    drop(guard);
    let (sender, receiver) = mpsc::sync_channel(1);
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
    drop(sender);
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
    fixture.publish(vec![output(1, b"before")], 1);
    assert_eq!(fixture.output(1), b"before");
    fixture.publish(vec![output(2, &[0xff]), output(3, &[0])], 3);
    let dropped = fixture
        .terminals
        .attachments
        .lock()
        .unwrap()
        .get(fixture.id.as_str())
        .unwrap()
        .receiver
        .as_ref()
        .unwrap()
        .try_recv()
        .unwrap();
    assert!(
        matches!(dropped.body, EventBody::TerminalOutput { sequence, .. } if sequence.value() == 2)
    );
    assert_eq!(fixture.output(3), [0xff, 0]);
    assert!(fixture.terminals.poll(&fixture.id).unwrap().is_empty());
}

#[test]
fn overflow_recovers_failed_status_without_conflating_it_with_disconnect() {
    let fixture = Fixture::new();
    let mut events: Vec<_> = (1..=600).map(|sequence| output(sequence, b"x")).collect();
    events.push(TerminalEvent::Failed("process failed".to_owned()));
    fixture.publish(events, 600);
    until(|| {
        for event in fixture.terminals.poll(&fixture.id).unwrap() {
            fixture.apply(event);
        }
        fixture.cache.lock().unwrap().status == ModelTerminalStatus::Failed
    });
    assert_eq!(fixture.cache.lock().unwrap().output, vec![b'x'; 600]);
    assert_eq!(fixture.snapshot().status, TerminalStatus::Failed);
    assert!(fixture.terminals.attachments.lock().unwrap().is_empty());
}

#[test]
fn a_busy_runtime_actor_does_not_block_poll_or_turn_disconnect_into_exit() {
    let fixture = Fixture::new();
    fixture
        .host
        .service()
        .disconnect(SessionId::try_new("desktop-1").unwrap())
        .unwrap();
    fixture.snapshot();
    let (entered_sender, entered_receiver) = mpsc::sync_channel(1);
    let (release_sender, release_receiver) = mpsc::sync_channel(1);
    *fixture.backend.blocked_input.lock().unwrap() = Some((entered_sender, release_receiver));
    let client = fixture.observer.clone();
    let terminal_id = lapis_client_api::TerminalId::try_new(fixture.id.as_str()).unwrap();
    let request = thread::spawn(move || {
        client.request(RequestBody::TerminalInput(TerminalInputRequest {
            terminal_id,
            data: "block".to_owned(),
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
    fixture.publish(vec![output(1, b"still running")], 1);
    assert_eq!(fixture.output(1), b"still running");
    assert_eq!(fixture.snapshot().status, TerminalStatus::Running);
}

#[test]
fn subscribed_events_at_snapshot_watermark_are_not_replayed() {
    let fixture = Fixture::new();
    fixture.publish(vec![output(1, b"before")], 1);
    assert_eq!(fixture.output(1), b"before");
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
    fixture.publish(vec![output(2, &[0xff]), output(3, &[0])], 3);
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
        if snapshot.output == [b"before".as_slice(), &[0xff, 0]].concat()
            && snapshot.output_sequence == 3
            && snapshot.status == ModelTerminalStatus::Running));
    for event in events {
        fixture.apply(event);
    }
    assert!(attachment.poll().unwrap().is_empty());
    drop(attachments);
    fixture.publish(vec![output(4, b"after")], 4);
    assert_eq!(fixture.output(4), b"after");
}

#[test]
fn overflow_recovers_authoritative_termination_and_releases_attachment() {
    let fixture = Fixture::new();
    fixture.publish(
        (1..=600).map(|sequence| output(sequence, b"x")).collect(),
        600,
    );
    fixture
        .observer
        .request(RequestBody::TerminalTerminate(TerminalTerminateRequest {
            terminal_id: lapis_client_api::TerminalId::try_new(fixture.id.as_str()).unwrap(),
        }))
        .unwrap();
    until(|| {
        for event in fixture.terminals.poll(&fixture.id).unwrap() {
            fixture.apply(event);
        }
        fixture.cache.lock().unwrap().status == ModelTerminalStatus::Exited
    });
    assert_eq!(fixture.cache.lock().unwrap().output, vec![b'x'; 600]);
    assert_eq!(fixture.snapshot().status, TerminalStatus::Terminated);
    assert!(fixture.terminals.attachments.lock().unwrap().is_empty());
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
        .input(&fixture.id, b"exactly once")
        .unwrap();
    assert_eq!(fixture.backend.inputs.load(Ordering::SeqCst), 1);
    fixture.terminals.resize(&fixture.id, 100, 40).unwrap();
    let snapshot = fixture.snapshot();
    assert_eq!(snapshot.status, TerminalStatus::Running);
    assert_eq!(
        snapshot.size,
        TerminalSize {
            columns: 100,
            rows: 40
        }
    );
    assert_eq!(fixture.backend.terminations.load(Ordering::SeqCst), 0);
    fixture
        .host
        .service()
        .disconnect(SessionId::try_new("desktop-1").unwrap())
        .unwrap();
    fixture.snapshot();
    fixture.terminals.terminate(&fixture.id).unwrap();
    assert_eq!(fixture.backend.inputs.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.backend.terminations.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.snapshot().status, TerminalStatus::Terminated);
}
use crate::BackendHost;
use lapis_backend_state::EmbeddedClient;
