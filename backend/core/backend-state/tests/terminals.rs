use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use lapis_backend_state::{
    BackendService, BackendSession, BackendState, WorkspaceEntry, WorkspaceFileBackend,
    WorkspaceRegistration,
};
use lapis_client_api::{
    EventBody, FORBIDDEN, INVALID_REQUEST, NOT_FOUND, RequestBody, RequestEnvelope, RequestId,
    ResponseBody, SessionId, SnapshotReason, SnapshotRequest, TerminalId, TerminalInputRequest,
    TerminalResizeRequest, TerminalSize, TerminalStartRequest, TerminalStatus,
    TerminalTerminateRequest, UNSUPPORTED, WorkspaceConnectRequest, WorkspaceId,
};
use lapis_document::{DocumentError, DocumentRepository, FileData, FileFingerprint};
use lapis_terminal::{
    TerminalBackend, TerminalError, TerminalEvent, TerminalId as BackendTerminalId,
};

#[derive(Default)]
struct EmptyFiles;

impl DocumentRepository for EmptyFiles {
    fn read_file(&self, _path: &Path) -> Result<FileData, DocumentError> {
        Err(DocumentError::io("terminal test does not expose files"))
    }

    fn write_file(
        &self,
        _path: &Path,
        _content: &[u8],
        _expected: Option<&FileFingerprint>,
    ) -> Result<FileFingerprint, DocumentError> {
        Err(DocumentError::io("terminal test does not expose files"))
    }

    fn fingerprint(&self, _path: &Path) -> Result<Option<FileFingerprint>, DocumentError> {
        Ok(None)
    }
}

impl WorkspaceFileBackend for EmptyFiles {
    fn list_children(&self, _directory: &Path) -> Result<Vec<WorkspaceEntry>, DocumentError> {
        Ok(Vec::new())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum FakeCall {
    Input { id: String, bytes: Vec<u8> },
    Resize { id: String, columns: u16, rows: u16 },
    Terminate { id: String },
}

#[derive(Default)]
struct FakeTerminalBackend {
    state: Mutex<FakeTerminalState>,
}

#[derive(Default)]
struct FakeTerminalState {
    next_id: u64,
    terminals: HashMap<String, FakeTerminal>,
    calls: Vec<FakeCall>,
}

struct FakeTerminal {
    cwd: PathBuf,
    columns: u16,
    rows: u16,
    events: VecDeque<TerminalEvent>,
}

impl FakeTerminalBackend {
    fn queue_output(&self, id: &BackendTerminalId, output: &str) {
        self.with_state(|state| {
            state
                .terminals
                .get_mut(id.as_str())
                .expect("terminal must exist")
                .events
                .push_back(TerminalEvent::Output(output.to_owned()));
        });
    }

    fn queue_exit(&self, id: &BackendTerminalId, code: Option<i32>) {
        self.with_state(|state| {
            state
                .terminals
                .get_mut(id.as_str())
                .expect("terminal must exist")
                .events
                .push_back(TerminalEvent::Exited { code });
        });
    }

    fn calls(&self) -> Vec<FakeCall> {
        self.with_state(|state| state.calls.clone())
    }

    fn with_state<T>(&self, operation: impl FnOnce(&mut FakeTerminalState) -> T) -> T {
        let mut state = self
            .state
            .lock()
            .expect("fake terminal state is not poisoned");
        operation(&mut state)
    }
}

impl TerminalBackend for FakeTerminalBackend {
    fn start(
        &self,
        cwd: &Path,
        columns: u16,
        rows: u16,
    ) -> Result<BackendTerminalId, TerminalError> {
        self.with_state(|state| {
            state.next_id += 1;
            let id = format!("fake-terminal-{}", state.next_id);
            state.terminals.insert(
                id.clone(),
                FakeTerminal {
                    cwd: cwd.to_owned(),
                    columns,
                    rows,
                    events: VecDeque::new(),
                },
            );
            Ok(BackendTerminalId::new(id))
        })
    }

    fn input(&self, id: &BackendTerminalId, bytes: &[u8]) -> Result<(), TerminalError> {
        self.with_state(|state| {
            let terminal = state
                .terminals
                .get(id.as_str())
                .ok_or_else(|| TerminalError::new("terminal not found"))?;
            let _ = (&terminal.cwd, terminal.columns, terminal.rows);
            state.calls.push(FakeCall::Input {
                id: id.as_str().to_owned(),
                bytes: bytes.to_vec(),
            });
            Ok(())
        })
    }

    fn resize(&self, id: &BackendTerminalId, columns: u16, rows: u16) -> Result<(), TerminalError> {
        self.with_state(|state| {
            let terminal = state
                .terminals
                .get_mut(id.as_str())
                .ok_or_else(|| TerminalError::new("terminal not found"))?;
            terminal.columns = columns;
            terminal.rows = rows;
            state.calls.push(FakeCall::Resize {
                id: id.as_str().to_owned(),
                columns,
                rows,
            });
            Ok(())
        })
    }

    fn poll(&self, id: &BackendTerminalId) -> Result<Vec<TerminalEvent>, TerminalError> {
        self.with_state(|state| {
            let terminal = state
                .terminals
                .get_mut(id.as_str())
                .ok_or_else(|| TerminalError::new("terminal not found"))?;
            Ok(terminal.events.drain(..).collect())
        })
    }

    fn terminate(&self, id: &BackendTerminalId) -> Result<(), TerminalError> {
        self.with_state(|state| {
            if !state.terminals.contains_key(id.as_str()) {
                return Err(TerminalError::new("terminal not found"));
            }
            state.calls.push(FakeCall::Terminate {
                id: id.as_str().to_owned(),
            });
            Ok(())
        })
    }
}

#[test]
fn terminal_lifecycle_polls_events_and_restores_from_snapshot() {
    let first_backend = Arc::new(FakeTerminalBackend::default());
    let second_backend = Arc::new(FakeTerminalBackend::default());
    let first_root = tempfile::tempdir().unwrap();
    let second_root = tempfile::tempdir().unwrap();
    let first_workspace = WorkspaceId::try_new("workspace-first").unwrap();
    let second_workspace = WorkspaceId::try_new("workspace-second").unwrap();
    let state = BackendState::new([
        WorkspaceRegistration::new(
            first_workspace.clone(),
            "First",
            first_root.path().to_owned(),
            Arc::new(EmptyFiles),
        )
        .with_terminal(first_backend.clone()),
        WorkspaceRegistration::new(
            second_workspace.clone(),
            "Second",
            second_root.path().to_owned(),
            Arc::new(EmptyFiles),
        )
        .with_terminal(second_backend.clone()),
    ])
    .unwrap();
    let service = BackendService::start(state).unwrap();
    let first_session = BackendSession::new(
        SessionId::try_new("session-first").unwrap(),
        first_workspace.clone(),
    );
    let second_session = BackendSession::new(
        SessionId::try_new("session-second").unwrap(),
        second_workspace.clone(),
    );
    connect(&service, &first_session, first_workspace.clone(), 1);
    connect(&service, &second_session, second_workspace.clone(), 2);
    let events = service.subscribe(first_session.clone()).unwrap();

    let ResponseBody::Error(unsupported) = dispatch(
        &service,
        &first_session,
        30,
        RequestBody::TerminalStart(TerminalStartRequest {
            workspace_id: first_workspace.clone(),
            cwd: None,
            command: Some("cargo test".to_owned()),
            size: TerminalSize {
                columns: 80,
                rows: 24,
            },
        }),
    ) else {
        panic!("terminal command must be explicitly unsupported");
    };
    assert_eq!(unsupported.code.as_str(), UNSUPPORTED);

    let ResponseBody::Error(invalid_size) = dispatch(
        &service,
        &first_session,
        31,
        RequestBody::TerminalStart(TerminalStartRequest {
            workspace_id: first_workspace.clone(),
            cwd: None,
            command: None,
            size: TerminalSize {
                columns: 0,
                rows: 24,
            },
        }),
    ) else {
        panic!("zero terminal size must be rejected");
    };
    assert_eq!(invalid_size.code.as_str(), INVALID_REQUEST);

    let ResponseBody::TerminalStart(started) = dispatch(
        &service,
        &first_session,
        3,
        RequestBody::TerminalStart(TerminalStartRequest {
            workspace_id: first_workspace.clone(),
            cwd: None,
            command: None,
            size: TerminalSize {
                columns: 80,
                rows: 24,
            },
        }),
    ) else {
        panic!("expected terminal start response");
    };
    assert_eq!(started.terminal.status, TerminalStatus::Running);
    let terminal_id = started.terminal.terminal_id.clone();
    assert_terminal_status(&events, &terminal_id, TerminalStatus::Running);

    assert!(matches!(
        dispatch(
            &service,
            &first_session,
            4,
            RequestBody::TerminalInput(TerminalInputRequest {
                terminal_id: terminal_id.clone(),
                data: "echo hello\n".to_owned(),
            }),
        ),
        ResponseBody::TerminalInput(_)
    ));
    assert!(matches!(
        dispatch(
            &service,
            &first_session,
            5,
            RequestBody::TerminalResize(TerminalResizeRequest {
                terminal_id: terminal_id.clone(),
                size: TerminalSize {
                    columns: 120,
                    rows: 36,
                },
            }),
        ),
        ResponseBody::TerminalResize(_)
    ));

    let backend_id = BackendTerminalId::new("fake-terminal-1");
    first_backend.queue_output(&backend_id, "hello\n");
    first_backend.queue_exit(&backend_id, Some(0));
    assert_terminal_output(&events, &terminal_id, "hello\n");
    assert_terminal_status(&events, &terminal_id, TerminalStatus::Exited);

    let ResponseBody::SnapshotResync(snapshot) = dispatch(
        &service,
        &first_session,
        6,
        RequestBody::SnapshotResync(SnapshotRequest {
            workspace_id: first_workspace.clone(),
            reason: SnapshotReason::Reconnect,
        }),
    ) else {
        panic!("expected terminal snapshot");
    };
    let terminal = snapshot
        .snapshot
        .terminals
        .iter()
        .find(|terminal| terminal.terminal_id == terminal_id)
        .expect("terminal must be in snapshot");
    assert_eq!(terminal.status, TerminalStatus::Exited);
    assert_eq!(terminal.buffered_output, "hello\n");

    assert!(matches!(
        dispatch(
            &service,
            &first_session,
            7,
            RequestBody::TerminalTerminate(TerminalTerminateRequest {
                terminal_id: terminal_id.clone(),
            }),
        ),
        ResponseBody::TerminalTerminate(_)
    ));
    assert!(first_backend.calls().contains(&FakeCall::Input {
        id: backend_id.as_str().to_owned(),
        bytes: b"echo hello\n".to_vec(),
    }));
    assert!(first_backend.calls().contains(&FakeCall::Resize {
        id: backend_id.as_str().to_owned(),
        columns: 120,
        rows: 36,
    }));

    let ResponseBody::Error(error) = dispatch(
        &service,
        &second_session,
        8,
        RequestBody::TerminalInput(TerminalInputRequest {
            terminal_id,
            data: "cross-workspace\n".to_owned(),
        }),
    ) else {
        panic!("cross-workspace terminal operation must be rejected");
    };
    assert!(matches!(error.code.as_str(), FORBIDDEN | NOT_FOUND));
    assert!(second_backend.calls().is_empty());

    let ResponseBody::TerminalStart(running) = dispatch(
        &service,
        &first_session,
        9,
        RequestBody::TerminalStart(TerminalStartRequest {
            workspace_id: first_workspace.clone(),
            cwd: None,
            command: None,
            size: TerminalSize {
                columns: 100,
                rows: 30,
            },
        }),
    ) else {
        panic!("expected second terminal start response");
    };
    let running_terminal_id = running.terminal.terminal_id;
    let running_backend_id = BackendTerminalId::new("fake-terminal-2");

    service
        .disconnect(first_session.session_id.clone())
        .unwrap();
    let reconnected = BackendSession::new(
        SessionId::try_new("session-reconnected").unwrap(),
        first_workspace.clone(),
    );
    connect(&service, &reconnected, first_workspace.clone(), 10);
    let ResponseBody::SnapshotResync(snapshot) = dispatch(
        &service,
        &reconnected,
        11,
        RequestBody::SnapshotResync(SnapshotRequest {
            workspace_id: first_workspace.clone(),
            reason: SnapshotReason::Reconnect,
        }),
    ) else {
        panic!("expected terminal snapshot after reconnect");
    };
    assert_eq!(snapshot.snapshot.terminals.len(), 2);
    let restored_exited = snapshot
        .snapshot
        .terminals
        .iter()
        .find(|terminal| terminal.terminal_id != running_terminal_id)
        .expect("exited terminal must remain in snapshot");
    assert_eq!(restored_exited.buffered_output, "hello\n");
    let restored_running = snapshot
        .snapshot
        .terminals
        .iter()
        .find(|terminal| terminal.terminal_id == running_terminal_id)
        .expect("running terminal must be in snapshot");
    assert_eq!(restored_running.status, TerminalStatus::Running);

    assert!(matches!(
        dispatch(
            &service,
            &reconnected,
            12,
            RequestBody::TerminalInput(TerminalInputRequest {
                terminal_id: running_terminal_id,
                data: "reconnected input\n".to_owned(),
            }),
        ),
        ResponseBody::TerminalInput(_)
    ));
    assert!(first_backend.calls().contains(&FakeCall::Input {
        id: running_backend_id.as_str().to_owned(),
        bytes: b"reconnected input\n".to_vec(),
    }));

    drop(events);
    drop(service);
    assert!(first_backend.calls().contains(&FakeCall::Terminate {
        id: running_backend_id.as_str().to_owned(),
    }));
}

fn connect(
    service: &BackendService,
    session: &BackendSession,
    workspace_id: WorkspaceId,
    request: u64,
) {
    assert!(matches!(
        dispatch(
            service,
            session,
            request,
            RequestBody::WorkspaceConnect(WorkspaceConnectRequest { workspace_id }),
        ),
        ResponseBody::WorkspaceConnect(_)
    ));
}

fn dispatch(
    service: &BackendService,
    session: &BackendSession,
    request: u64,
    body: RequestBody,
) -> ResponseBody {
    service
        .dispatch(
            session.clone(),
            RequestEnvelope {
                request_id: RequestId::try_new(format!("terminal-request-{request}")).unwrap(),
                body,
            },
        )
        .unwrap()
        .body
}

fn assert_terminal_status(
    events: &lapis_backend_state::BackendEventReceiver,
    terminal_id: &TerminalId,
    expected: TerminalStatus,
) {
    wait_for_event(events, |event| {
        matches!(
            &event.body,
            EventBody::TerminalStatus {
                terminal_id: event_terminal_id,
                status,
            } if event_terminal_id == terminal_id && *status == expected
        )
    });
}

fn assert_terminal_output(
    events: &lapis_backend_state::BackendEventReceiver,
    terminal_id: &TerminalId,
    expected: &str,
) {
    wait_for_event(events, |event| {
        matches!(
            &event.body,
            EventBody::TerminalOutput {
                terminal_id: event_terminal_id,
                data,
                ..
            } if event_terminal_id == terminal_id && data == expected
        )
    });
}

fn wait_for_event(
    events: &lapis_backend_state::BackendEventReceiver,
    predicate: impl Fn(&lapis_client_api::EventEnvelope) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "expected terminal event was not published"
        );
        if let Ok(event) = events.recv_timeout(remaining.min(Duration::from_millis(100)))
            && predicate(&event)
        {
            return;
        }
    }
}
