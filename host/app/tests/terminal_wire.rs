//! `BackendClient::terminals()` が in-process Host で Terminal を開始・読み取りできる。

use std::time::{Duration, Instant};

use terminal::{
    TerminalBackend, TerminalEvent, TerminalId, TerminalInput, TerminalKey, TerminalKeyEvent,
    TerminalRevision, TerminalScreen, TerminalSnapshot,
};

struct Screen {
    model: TerminalScreen,
    revision: TerminalRevision,
}

impl Screen {
    fn from_snapshot(snapshot: TerminalSnapshot) -> Self {
        Self {
            model: snapshot.screen,
            revision: snapshot.revision,
        }
    }

    fn apply(&mut self, events: Vec<TerminalEvent>) {
        for event in events {
            match event {
                TerminalEvent::Delta(delta) if delta.revision == self.revision + 1 => {
                    self.model.apply_delta(&delta).unwrap();
                    self.revision = delta.revision;
                }
                TerminalEvent::Snapshot(snapshot) if snapshot.revision >= self.revision => {
                    self.model = snapshot.screen;
                    self.revision = snapshot.revision;
                }
                _ => {}
            }
        }
    }
}

fn read_until(terminals: &dyn TerminalBackend, id: &TerminalId, screen: &mut Screen, needle: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        screen.apply(terminals.poll(id).unwrap());
        let text = screen.model.plain_text();
        if text.contains(needle) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "terminal output did not contain {needle:?}: {text:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn embedded_backend_client_terminals_start_and_read_a_session() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let host = host::BackendHost::start().unwrap();
    host.register_workspace(&root).unwrap();
    let terminals = host::BackendClient::from(host.clone()).terminals();

    let profiles = terminals.profiles(&root).unwrap();
    let profile = profiles.default.clone().expect("default profile is set");
    assert!(profiles.find(&profile).is_some());

    let id = terminals
        .start_profile(&root, 160, 40, Some(&profile))
        .unwrap();
    let snapshot = terminals.attach(&root, &id).unwrap();
    assert_eq!(snapshot.id, id);
    assert_eq!(snapshot.cwd, root);
    let mut screen = Screen::from_snapshot(snapshot);

    // ConPTY の初期化問い合わせが終わるまで待ってから入力する。
    let deadline = Instant::now() + Duration::from_secs(10);
    while screen.model.plain_text().trim().is_empty() {
        screen.apply(terminals.poll(&id).unwrap());
        assert!(Instant::now() < deadline, "terminal did not become ready");
        std::thread::sleep(Duration::from_millis(20));
    }
    terminals
        .input(&id, &TerminalInput::Text("echo lapis-wire".to_owned()))
        .unwrap();
    terminals
        .input(
            &id,
            &TerminalInput::Key(TerminalKeyEvent {
                key: TerminalKey::Enter,
                modifiers: Default::default(),
            }),
        )
        .unwrap();
    read_until(terminals.as_ref(), &id, &mut screen, "lapis-wire");

    terminals.terminate(&id).unwrap();
    assert!(terminals.list(&root).unwrap().is_empty());
    assert!(terminals.poll(&id).is_err());
}

#[test]
fn embedded_terminals_emit_change_notifications_while_sessions_exist() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let host = host::BackendHost::start().unwrap();
    host.register_workspace(&root).unwrap();
    let terminals = host::BackendClient::from(host).terminals();
    let mut changes = terminals.changes().expect("changes is available");
    // changes() は購読を 1 本だけ返す。UI と同じく 1 度だけ取得する。
    assert!(terminals.changes().is_none());

    let id = terminals.start(&root, 80, 24).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if changes.try_recv().is_ok() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "terminal session did not emit a change notification"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    terminals.terminate(&id).unwrap();
}
