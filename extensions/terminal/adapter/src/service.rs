//! Host 側 Terminal セッションサービス。
//!
//! PTY は `TerminalHost` port だけを通して扱い、出力 bytes は `terminal-renderer` の
//! 画面モデルで差分へ変換する。Workspace 境界・操作権・接続ごとの読み取り位置は
//! この service が所有する。Desktop の in-process Host とリモート接続は同じ service
//! を使い、接続経路だけが違う。

use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use terminal::{
    TerminalDelta, TerminalError, TerminalEvent, TerminalId, TerminalInput, TerminalProfile,
    TerminalProfileId, TerminalProfiles, TerminalRevision, TerminalSize, TerminalSnapshot,
    TerminalStatus,
};
use terminal_host::{TerminalHost, process_info, sessions};
use terminal_renderer::{TerminalEngine, normalized_size};

/// 保持する差分の上限。これより古い読み取り位置は snapshot で再同期する。
const MAX_DELTA_HISTORY: usize = 128;
/// 1 回の読み取りで受け取る上限。`terminal-session-api` の dispatch 上限に合わせる。
const MAX_READ_BYTES: u32 = 8 * 1024;

/// 接続ごとの読み取り位置と、最後に通知した操作権。
struct Reader {
    revision: TerminalRevision,
    owner: Option<Option<String>>,
}

struct Live {
    workspace: String,
    cwd: PathBuf,
    id: TerminalId,
    engine: TerminalEngine,
    revision: TerminalRevision,
    next: u64,
    history: VecDeque<TerminalDelta>,
    readers: HashMap<String, Reader>,
    owner: Option<String>,
    exited: bool,
    failed: bool,
    failure_reported: bool,
}

impl Live {
    fn snapshot(&self) -> TerminalSnapshot {
        TerminalSnapshot {
            id: self.id.clone(),
            cwd: self.cwd.clone(),
            status: if self.exited {
                TerminalStatus::Exited
            } else if self.failed {
                TerminalStatus::Failed
            } else {
                TerminalStatus::Running
            },
            revision: self.revision,
            screen: self.engine.snapshot().clone(),
        }
    }
}

struct State {
    sessions: BTreeMap<String, Live>,
    /// 終了後に `forget` できなかった PTY。次の操作で回収を再試行する。
    reaping: Vec<String>,
}

/// Workspace ごとの Terminal セッション。`Arc` で共有する。
pub struct TerminalService {
    host: Arc<dyn TerminalHost + Send + Sync>,
    state: Mutex<State>,
}

impl TerminalService {
    pub fn new<H>(host: H) -> Self
    where
        H: TerminalHost + Send + Sync + 'static,
    {
        Self {
            host: Arc::new(host),
            state: Mutex::new(State {
                sessions: BTreeMap::new(),
                reaping: Vec::new(),
            }),
        }
    }

    fn lock(&self) -> Result<MutexGuard<'_, State>, TerminalError> {
        self.state
            .lock()
            .map_err(|_| TerminalError::new("Terminal state lock failed"))
    }

    fn reap(&self, state: &mut State) {
        let host = Arc::clone(&self.host);
        state.reaping.retain(|id| host.forget(id).is_err());
    }

    fn session<'a>(
        state: &'a mut State,
        workspace: &str,
        id: &TerminalId,
    ) -> Result<&'a mut Live, TerminalError> {
        state
            .sessions
            .get_mut(id.as_str())
            .filter(|session| session.workspace == workspace)
            .ok_or_else(|| TerminalError::new("Terminal session is not attached"))
    }

    fn require_owner(session: &Live, caller: &str) -> Result<(), TerminalError> {
        if session
            .owner
            .as_deref()
            .is_some_and(|owner| owner != caller)
        {
            return Err(TerminalError::new("Terminal is owned by another session"));
        }
        Ok(())
    }

    fn close(&self, state: &mut State, id: &str) -> Result<(), TerminalError> {
        self.host.kill(id).map_err(TerminalError::new)?;
        self.host.detach(id).map_err(TerminalError::new)?;
        state.sessions.remove(id);
        state.reaping.push(id.to_owned());
        self.reap(state);
        Ok(())
    }

    fn profiles(&self) -> Result<TerminalProfiles, TerminalError> {
        let information = self.host.information().map_err(TerminalError::new)?;
        let mut profiles = match information.pty {
            process_info::PtyFlavor::Conpty => {
                vec![TerminalProfile::new("cmd", "Command Prompt", "cmd.exe").with_args(["/D"])]
            }
            process_info::PtyFlavor::Posix => vec![
                TerminalProfile::new("bash", "bash", "bash"),
                TerminalProfile::new("zsh", "zsh", "zsh"),
                TerminalProfile::new("fish", "fish", "fish"),
                TerminalProfile::new("pwsh", "PowerShell", "pwsh"),
            ],
        };
        let default = match information.pty {
            process_info::PtyFlavor::Conpty => "powershell",
            process_info::PtyFlavor::Posix => "login-shell",
        };
        profiles.insert(
            0,
            TerminalProfile {
                id: TerminalProfileId::new(default),
                name: if default == "powershell" {
                    "PowerShell"
                } else {
                    "Login Shell"
                }
                .to_owned(),
                program: information.default_shell.program,
                args: information.default_shell.arguments,
                env: Default::default(),
            },
        );
        Ok(TerminalProfiles::new(
            profiles,
            Some(TerminalProfileId::new(default)),
        ))
    }

    /// Host が許可する profile で Terminal を開始する。
    pub fn start(
        &self,
        workspace: &str,
        caller: &str,
        cwd: &Path,
        columns: u16,
        rows: u16,
        profile: Option<&TerminalProfileId>,
    ) -> Result<TerminalId, TerminalError> {
        if workspace.is_empty() || caller.is_empty() {
            return Err(TerminalError::new("missing authenticated context"));
        }
        let profiles = self.profiles()?;
        let profile = profiles
            .resolve(profile)?
            .ok_or_else(|| TerminalError::new("No Terminal profile is available"))?
            .clone();
        let size = normalized_size(TerminalSize::new(columns, rows));
        let pty = self
            .host
            .spawn(&sessions::SpawnRequest {
                program: profile.program.clone(),
                arguments: profile.args.clone(),
                environment: profile
                    .env
                    .iter()
                    .map(|(name, value)| sessions::EnvironmentEntry {
                        name: name.clone(),
                        value: value.clone(),
                    })
                    .collect(),
                working_directory: path_string(cwd)?,
                initial_size: sessions::Size {
                    columns: size.columns,
                    rows: size.rows,
                },
            })
            .map_err(TerminalError::new)?;
        if let Err(error) = self.host.attach(&pty) {
            let _ = self.host.kill(&pty);
            let _ = self.host.detach(&pty);
            return Err(TerminalError::new(error));
        }
        let id = TerminalId::new(pty.clone());
        let engine = TerminalEngine::new(
            size,
            Box::new(HostWriter {
                host: Arc::clone(&self.host),
                id: pty,
            }),
        );
        let mut state = self.lock()?;
        self.reap(&mut state);
        state.sessions.insert(
            id.as_str().to_owned(),
            Live {
                workspace: workspace.to_owned(),
                cwd: cwd.to_owned(),
                id: id.clone(),
                engine,
                revision: 0,
                next: 0,
                history: VecDeque::new(),
                readers: HashMap::new(),
                owner: Some(caller.to_owned()),
                exited: false,
                failed: false,
                failure_reported: false,
            },
        );
        Ok(id)
    }

    /// Host が許可する profile 一覧。
    pub fn profiles_for(
        &self,
        workspace: &str,
        _cwd: &Path,
    ) -> Result<TerminalProfiles, TerminalError> {
        if workspace.is_empty() {
            return Err(TerminalError::new("missing authenticated context"));
        }
        self.profiles()
    }

    /// この Workspace で稼働中の Terminal。接続先を問わず同じ一覧を返す。
    pub fn list(
        &self,
        workspace: &str,
        _cwd: &Path,
    ) -> Result<Vec<TerminalSnapshot>, TerminalError> {
        if workspace.is_empty() {
            return Err(TerminalError::new("missing authenticated context"));
        }
        let mut state = self.lock()?;
        self.reap(&mut state);
        Ok(state
            .sessions
            .values()
            .filter(|session| session.workspace == workspace)
            .map(Live::snapshot)
            .collect())
    }

    /// 再接続した Client へ現在の画面を返す。読み取り位置は現在の版に合わせる。
    pub fn attach(
        &self,
        workspace: &str,
        caller: &str,
        id: &TerminalId,
    ) -> Result<TerminalSnapshot, TerminalError> {
        let mut state = self.lock()?;
        self.reap(&mut state);
        let session = Self::session(&mut state, workspace, id)?;
        session.readers.insert(
            caller.to_owned(),
            Reader {
                revision: session.revision,
                owner: None,
            },
        );
        Ok(session.snapshot())
    }

    pub fn input(
        &self,
        workspace: &str,
        caller: &str,
        id: &TerminalId,
        input: &TerminalInput,
    ) -> Result<(), TerminalError> {
        let mut state = self.lock()?;
        let session = Self::session(&mut state, workspace, id)?;
        Self::require_owner(session, caller)?;
        match input {
            // 文字列は 1 回の write にまとめる。PTY の入力キューは有限なので、
            // 1 文字ずつ書くと取りこぼすことがある。
            TerminalInput::Text(text) => self
                .host
                .write(id.as_str(), text.as_bytes())
                .map_err(TerminalError::new),
            _ => session.engine.input(input),
        }
    }

    pub fn resize(
        &self,
        workspace: &str,
        caller: &str,
        id: &TerminalId,
        columns: u16,
        rows: u16,
    ) -> Result<(), TerminalError> {
        let mut state = self.lock()?;
        let session = Self::session(&mut state, workspace, id)?;
        Self::require_owner(session, caller)?;
        let size = normalized_size(TerminalSize::new(columns, rows));
        self.host
            .resize(
                id.as_str(),
                sessions::Size {
                    columns: size.columns,
                    rows: size.rows,
                },
            )
            .map_err(TerminalError::new)?;
        let (_, delta) = session.engine.resize(size);
        session.revision = delta.revision;
        session.history.push_back(delta);
        trim_history(session);
        Ok(())
    }

    /// 前回の `poll` 以降に起きた画面差分・終了・操作権の変化を返す。
    pub fn poll(
        &self,
        workspace: &str,
        caller: &str,
        id: &TerminalId,
    ) -> Result<Vec<TerminalEvent>, TerminalError> {
        let mut state = self.lock()?;
        self.reap(&mut state);
        let session = Self::session(&mut state, workspace, id)?;
        let output = self
            .host
            .read(id.as_str(), session.next, MAX_READ_BYTES, 0)
            .map_err(TerminalError::new)?;
        let lost = output.lost_before.is_some();
        if !output.bytes.is_empty() {
            let delta = session.engine.process(&output.bytes);
            session.revision = delta.revision;
            if !lost {
                session.history.push_back(delta);
                trim_history(session);
            }
        }
        session.next = output.next;
        if lost {
            session.history.clear();
            session.readers.clear();
        }
        if output.read_error.is_some() || output.write_error.is_some() {
            session.failed = true;
        }

        let previous = session.readers.get(caller);
        let revision = previous.map_or(0, |reader| reader.revision);
        let seen_owner = previous.and_then(|reader| reader.owner.clone());
        let stale = revision != session.revision
            && session
                .history
                .front()
                .is_some_and(|delta| delta.revision > revision.saturating_add(1));

        let mut events = Vec::new();
        if lost || stale {
            events.push(TerminalEvent::Snapshot(session.snapshot()));
        } else {
            events.extend(
                session
                    .history
                    .iter()
                    .filter(|delta| delta.revision > revision)
                    .cloned()
                    .map(TerminalEvent::Delta),
            );
        }
        let owner = session.owner.clone();
        if seen_owner.as_ref() != Some(&owner) {
            events.push(TerminalEvent::Owner {
                owner: owner.clone(),
            });
        }
        session.readers.insert(
            caller.to_owned(),
            Reader {
                revision: session.revision,
                owner: Some(owner),
            },
        );

        if let Some(exit) = output.exit
            && !session.exited
        {
            session.exited = true;
            events.push(TerminalEvent::Exited { code: exit.code });
        }
        if let Some(error) = output.read_error.or(output.write_error)
            && !session.failure_reported
        {
            session.failure_reported = true;
            events.push(TerminalEvent::Failed(error));
        }
        Ok(events)
    }

    pub fn terminate(
        &self,
        workspace: &str,
        _caller: &str,
        id: &TerminalId,
    ) -> Result<(), TerminalError> {
        let mut state = self.lock()?;
        Self::session(&mut state, workspace, id)?;
        self.close(&mut state, id.as_str())
    }

    /// 操作権を取得する。他の Client が所有中でも奪う。
    pub fn claim(
        &self,
        workspace: &str,
        caller: &str,
        id: &TerminalId,
    ) -> Result<(), TerminalError> {
        let mut state = self.lock()?;
        let session = Self::session(&mut state, workspace, id)?;
        session.owner = Some(caller.to_owned());
        Ok(())
    }

    /// 操作権を手放す。他の Client が所有中なら拒否する。
    pub fn release(
        &self,
        workspace: &str,
        caller: &str,
        id: &TerminalId,
    ) -> Result<(), TerminalError> {
        let mut state = self.lock()?;
        let session = Self::session(&mut state, workspace, id)?;
        if session
            .owner
            .as_deref()
            .is_some_and(|owner| owner != caller)
        {
            return Err(TerminalError::new("Terminal is owned by another session"));
        }
        if session.owner.as_deref() == Some(caller) {
            session.owner = None;
        }
        Ok(())
    }

    /// 切断した接続の操作権を解放する。PTY は残し、別の接続が再アタッチできる。
    pub fn release_session(&self, caller: &str) -> Result<(), TerminalError> {
        let mut state = self.lock()?;
        for session in state.sessions.values_mut() {
            if session.owner.as_deref() == Some(caller) {
                session.owner = None;
            }
            session.readers.remove(caller);
        }
        Ok(())
    }

    /// Workspace の Terminal を終了する。Workspace の破棄時に使う。
    pub fn close_workspace(&self, workspace: &str) -> Result<(), TerminalError> {
        let mut state = self.lock()?;
        let ids = state
            .sessions
            .iter()
            .filter(|(_, session)| session.workspace == workspace)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in ids {
            self.close(&mut state, &id)?;
        }
        Ok(())
    }
}

struct HostWriter {
    host: Arc<dyn TerminalHost + Send + Sync>,
    id: String,
}

impl Write for HostWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.host.write(&self.id, data).map_err(io::Error::other)?;
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn path_string(path: &Path) -> Result<String, TerminalError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| TerminalError::new("Terminal working directory is not UTF-8"))
}

fn trim_history(session: &mut Live) {
    while session.history.len() > MAX_DELTA_HISTORY {
        session.history.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LocalTerminalHost;
    use local_process::pty::LocalPtyService;
    use std::time::{Duration, Instant};
    use terminal::{TerminalKey, TerminalKeyEvent, TerminalScreen};

    fn service() -> TerminalService {
        #[cfg(windows)]
        let host = LocalTerminalHost::with_shell(
            Arc::new(LocalPtyService::default()),
            "cmd.exe",
            vec!["/D".to_owned()],
        );
        #[cfg(unix)]
        let host = LocalTerminalHost::with_shell(
            Arc::new(LocalPtyService::default()),
            "/bin/sh",
            Vec::new(),
        );
        TerminalService::new(host)
    }

    #[cfg(windows)]
    fn profile() -> Option<TerminalProfileId> {
        Some(TerminalProfileId::new("cmd"))
    }

    #[cfg(unix)]
    fn profile() -> Option<TerminalProfileId> {
        None
    }

    struct Screen {
        model: TerminalScreen,
        revision: TerminalRevision,
    }

    impl Screen {
        fn new(columns: u16, rows: u16) -> Self {
            Self {
                model: TerminalScreen::empty(TerminalSize::new(columns, rows)),
                revision: 0,
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

    fn wait_for(
        service: &TerminalService,
        caller: &str,
        id: &TerminalId,
        screen: &mut Screen,
        needle: &str,
    ) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            screen.apply(service.poll("workspace-1", caller, id).unwrap());
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

    /// ConPTY は接続直後に端末へ問い合わせる。応答が返るまで入力は捨てられるため、
    /// 最初の出力（shell の banner か prompt）を待ってから入力を送る。
    fn wait_ready(service: &TerminalService, caller: &str, id: &TerminalId, screen: &mut Screen) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            screen.apply(service.poll("workspace-1", caller, id).unwrap());
            if !screen.model.plain_text().trim().is_empty() {
                return;
            }
            assert!(Instant::now() < deadline, "terminal did not become ready");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn type_line(service: &TerminalService, caller: &str, id: &TerminalId, line: &str) {
        service
            .input(
                "workspace-1",
                caller,
                id,
                &TerminalInput::Text(line.to_owned()),
            )
            .unwrap();
        service
            .input(
                "workspace-1",
                caller,
                id,
                &TerminalInput::Key(TerminalKeyEvent {
                    key: TerminalKey::Enter,
                    modifiers: Default::default(),
                }),
            )
            .unwrap();
    }

    #[test]
    fn profiles_are_platform_specific_and_reject_unknown_ids() {
        let service = service();
        let profiles = service.profiles_for("workspace-1", Path::new(".")).unwrap();
        assert!(!profiles.profiles.is_empty());
        let default = profiles.default.as_ref().expect("default profile is set");
        assert!(
            profiles.find(default).is_some(),
            "default must be published"
        );
        #[cfg(windows)]
        assert_eq!(default.as_str(), "powershell");
        #[cfg(unix)]
        assert_eq!(default.as_str(), "login-shell");
        assert!(
            service
                .start(
                    "workspace-1",
                    "client-a",
                    Path::new("."),
                    160,
                    40,
                    Some(&TerminalProfileId::new("missing")),
                )
                .is_err()
        );
    }

    #[test]
    fn sessions_start_accept_input_poll_resize_and_terminate() {
        let service = service();
        let id = service
            .start(
                "workspace-1",
                "client-a",
                Path::new("."),
                160,
                40,
                profile().as_ref(),
            )
            .unwrap();
        assert_eq!(
            service.list("workspace-1", Path::new(".")).unwrap().len(),
            1
        );
        assert!(
            service
                .list("workspace-2", Path::new("."))
                .unwrap()
                .is_empty()
        );

        let mut screen = Screen::new(160, 40);
        wait_ready(&service, "client-a", &id, &mut screen);
        type_line(&service, "client-a", &id, "echo lapis-terminal-service");
        wait_for(
            &service,
            "client-a",
            &id,
            &mut screen,
            "lapis-terminal-service",
        );

        // 操作権は claim で移り、release で戻る。所有者以外の入力は拒否する。
        service.claim("workspace-1", "client-b", &id).unwrap();
        assert!(
            service
                .input(
                    "workspace-1",
                    "client-a",
                    &id,
                    &TerminalInput::Text("x".to_owned())
                )
                .is_err()
        );
        assert!(service.release("workspace-1", "client-a", &id).is_err());
        service.release("workspace-1", "client-b", &id).unwrap();

        service
            .resize("workspace-1", "client-a", &id, 120, 30)
            .unwrap();
        let mut resized = false;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && !resized {
            for event in service.poll("workspace-1", "client-a", &id).unwrap() {
                if let TerminalEvent::Delta(delta) = &event
                    && delta.size == TerminalSize::new(120, 30)
                {
                    resized = true;
                }
                screen.apply(vec![event]);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(resized, "resize must publish a delta with the new size");

        type_line(&service, "client-a", &id, "echo lapis-owned");
        wait_for(&service, "client-a", &id, &mut screen, "lapis-owned");

        service.terminate("workspace-1", "client-a", &id).unwrap();
        assert!(
            service
                .list("workspace-1", Path::new("."))
                .unwrap()
                .is_empty()
        );
        assert!(service.poll("workspace-1", "client-a", &id).is_err());
    }

    #[test]
    fn every_reader_receives_the_new_session_deltas() {
        let service = service();
        let id = service
            .start(
                "workspace-1",
                "client-a",
                Path::new("."),
                160,
                40,
                profile().as_ref(),
            )
            .unwrap();
        let mut first = Screen::new(160, 40);
        wait_ready(&service, "client-a", &id, &mut first);
        // 後から接続した reader は attach の snapshot を起点にする。
        let snapshot = service.attach("workspace-1", "client-b", &id).unwrap();
        let mut second = Screen {
            model: snapshot.screen,
            revision: snapshot.revision,
        };
        type_line(&service, "client-a", &id, "echo lapis-shared");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            first.apply(service.poll("workspace-1", "client-a", &id).unwrap());
            second.apply(service.poll("workspace-1", "client-b", &id).unwrap());
            if first.model.plain_text().contains("lapis-shared")
                && second.model.plain_text().contains("lapis-shared")
            {
                break;
            }
            assert!(Instant::now() < deadline, "both readers must see output");
            std::thread::sleep(Duration::from_millis(20));
        }
        service.release_session("client-a").unwrap();
        service.release_session("client-b").unwrap();
        // Workspace の破棄では Terminal も終了する。
        service.close_workspace("workspace-1").unwrap();
        assert!(
            service
                .list("workspace-1", Path::new("."))
                .unwrap()
                .is_empty()
        );
        assert!(service.poll("workspace-1", "client-a", &id).is_err());
    }
}
