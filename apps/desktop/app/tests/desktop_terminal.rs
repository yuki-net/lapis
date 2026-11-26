//! Desktop が使う内部 Terminal が実 PTY で動くことを確認する。

use std::time::{Duration, Instant};

use terminal::{
    TerminalBackend, TerminalEvent, TerminalId, TerminalInput, TerminalKey, TerminalKeyEvent,
    TerminalRevision, TerminalScreen, TerminalSize, TerminalSnapshot,
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

fn read_until(
    terminals: &dyn TerminalBackend,
    id: &TerminalId,
    screen: &mut Screen,
    check: impl Fn(&TerminalScreen) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        screen.apply(terminals.poll(id).unwrap());
        if check(&screen.model) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "terminal output did not reach the expected state: {:?}",
            screen.model.plain_text()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "実 PTY を起動するため CI の専用ステップで実行する"]
fn desktop_terminal_runs_internal_pty() {
    let state = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path().canonicalize().unwrap();
    let host = host::BackendHost::start_with_state_directory(state.path().to_owned()).unwrap();
    host.register_workspace(&root).unwrap();
    let terminals = host::BackendClient::from(host).terminals();

    let profiles = terminals.profiles(&root).unwrap();
    let profile = profiles.default.clone().expect("default profile is set");
    assert!(profiles.find(&profile).is_some());

    let id = terminals
        .start_profile(&root, 80, 24, Some(&profile))
        .unwrap();
    let mut screen = Screen::from_snapshot(terminals.attach(&root, &id).unwrap());

    // ConPTY の初期化問い合わせが終わるまで待ってから入力する。
    read_until(terminals.as_ref(), &id, &mut screen, |screen| {
        !screen.plain_text().trim().is_empty()
    });

    terminals
        .input(&id, &TerminalInput::Text("echo lapis-internal".to_owned()))
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
    read_until(terminals.as_ref(), &id, &mut screen, |screen| {
        screen.plain_text().contains("lapis-internal")
    });

    terminals.resize(&id, 100, 30).unwrap();
    read_until(terminals.as_ref(), &id, &mut screen, |screen| {
        screen.size == TerminalSize::new(100, 30)
    });

    terminals.terminate(&id).unwrap();
    assert!(terminals.list(&root).unwrap().is_empty());
    assert!(terminals.poll(&id).is_err());
}
