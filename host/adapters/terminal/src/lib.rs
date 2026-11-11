//! Local Host の PTY を使う Terminal backend。

use std::{
    collections::{HashMap, VecDeque},
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
};

use futures_channel::mpsc as futures_mpsc;
use terminal::{
    TerminalBackend, TerminalDelta, TerminalError, TerminalEvent, TerminalId, TerminalInput,
    TerminalProfile, TerminalProfileId, TerminalProfiles, TerminalSize,
};

mod terminal_engine;
use terminal_engine::{TerminalEngine, normalized_size};

/// OS ごとの既定 profile。Host はこれを設定として差し替えられる。
pub fn default_profiles() -> TerminalProfiles {
    #[cfg(windows)]
    {
        TerminalProfiles::new(
            vec![
                TerminalProfile::new("powershell", "PowerShell", "powershell.exe")
                    .with_args(["-NoLogo"]),
                TerminalProfile::new("cmd", "Command Prompt", "cmd.exe").with_args(["/D"]),
            ],
            Some(TerminalProfileId::new("powershell")),
        )
    }
    #[cfg(target_os = "macos")]
    {
        let login = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_owned());
        TerminalProfiles::new(
            vec![
                TerminalProfile::new("login-shell", "Login Shell", login),
                TerminalProfile::new("zsh", "zsh", "zsh"),
                TerminalProfile::new("bash", "bash", "bash"),
                TerminalProfile::new("fish", "fish", "fish"),
                TerminalProfile::new("pwsh", "PowerShell", "pwsh"),
            ],
            Some(TerminalProfileId::new("login-shell")),
        )
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let login = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
        TerminalProfiles::new(
            vec![
                TerminalProfile::new("login-shell", "Login Shell", login),
                TerminalProfile::new("bash", "bash", "bash"),
                TerminalProfile::new("zsh", "zsh", "zsh"),
                TerminalProfile::new("fish", "fish", "fish"),
                TerminalProfile::new("pwsh", "PowerShell", "pwsh"),
            ],
            Some(TerminalProfileId::new("login-shell")),
        )
    }
}

struct LiveTerminal {
    master: Box<dyn portable_pty::MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send>,
    receiver: mpsc::Receiver<RawTerminalEvent>,
    engine: TerminalEngine,
    pending_events: VecDeque<TerminalEvent>,
    reader_finished: Arc<AtomicBool>,
    exited: bool,
}

enum RawTerminalEvent {
    Output(Vec<u8>),
    Failed(String),
}

#[derive(Clone)]
pub struct LocalTerminalBackend {
    terminals: Arc<Mutex<HashMap<String, LiveTerminal>>>,
    next_id: Arc<AtomicU64>,
    profiles: Arc<TerminalProfiles>,
    changes: ChangeNotifier,
}

/// PTY reader から Runtime への起床通知。receiver は 1 度だけ渡す。
#[derive(Clone)]
struct ChangeNotifier {
    sender: futures_mpsc::UnboundedSender<()>,
    receiver: Arc<Mutex<Option<futures_mpsc::UnboundedReceiver<()>>>>,
}

impl ChangeNotifier {
    fn new() -> Self {
        let (sender, receiver) = futures_mpsc::unbounded();
        Self {
            sender,
            receiver: Arc::new(Mutex::new(Some(receiver))),
        }
    }

    fn notify(&self) {
        let _ = self.sender.unbounded_send(());
    }

    fn take(&self) -> Option<futures_mpsc::UnboundedReceiver<()>> {
        self.receiver.lock().ok()?.take()
    }
}

impl Default for LocalTerminalBackend {
    fn default() -> Self {
        Self::with_profiles(default_profiles())
    }
}

impl LocalTerminalBackend {
    /// Host 設定で許可された profile だけを起動できる backend を作る。
    pub fn with_profiles(profiles: TerminalProfiles) -> Self {
        Self {
            terminals: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(AtomicU64::new(0)),
            profiles: Arc::new(profiles),
            changes: ChangeNotifier::new(),
        }
    }

    fn lock(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, HashMap<String, LiveTerminal>>, TerminalError> {
        self.terminals
            .lock()
            .map_err(|_| TerminalError::new("Terminal state lock failed"))
    }
}

fn read_terminal_stream<R: Read>(
    mut reader: R,
    sender: mpsc::Sender<RawTerminalEvent>,
    notify: impl Fn(),
) {
    let mut buffer = [0; 4096];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(length) => {
                if sender
                    .send(RawTerminalEvent::Output(buffer[..length].to_vec()))
                    .is_err()
                {
                    break;
                }
                notify();
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::UnexpectedEof
                ) =>
            {
                break;
            }
            Err(error) => {
                let _ = sender.send(RawTerminalEvent::Failed(error.to_string()));
                break;
            }
        }
    }
}

impl TerminalBackend for LocalTerminalBackend {
    fn start(&self, cwd: &Path, columns: u16, rows: u16) -> Result<TerminalId, TerminalError> {
        self.start_profile(cwd, columns, rows, None)
    }

    fn start_profile(
        &self,
        cwd: &Path,
        columns: u16,
        rows: u16,
        profile: Option<&TerminalProfileId>,
    ) -> Result<TerminalId, TerminalError> {
        let size = normalized_size(TerminalSize::new(columns, rows));
        let profile = self
            .profiles
            .resolve(profile)?
            .ok_or_else(|| TerminalError::new("利用できるTerminal profileがありません"))?
            .clone();
        let cwd = shell_directory(cwd, &profile.program)?;
        let mut command = portable_pty::CommandBuilder::new(&profile.program);
        for argument in &profile.args {
            command.arg(argument);
        }
        for (key, value) in &profile.env {
            command.env(key, value);
        }
        command.cwd(cwd);
        let pty_system = portable_pty::native_pty_system();
        let pair = pty_system
            .openpty(portable_pty::PtySize {
                rows: size.rows,
                cols: size.columns,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| TerminalError::new(error.to_string()))?;
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| TerminalError::new(error.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| TerminalError::new(error.to_string()))?;
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| TerminalError::new(error.to_string()))?;
        let (sender, receiver) = mpsc::channel();
        let reader_finished = Arc::new(AtomicBool::new(false));
        let reader_finished_for_thread = reader_finished.clone();
        let changes = self.changes.clone();
        thread::spawn(move || {
            // 出力の到着と reader 終了（PTY の EOF / 失敗）で Runtime を起床させる。
            read_terminal_stream(reader, sender, || changes.notify());
            reader_finished_for_thread.store(true, Ordering::Release);
            changes.notify();
        });
        let id = TerminalId::new(format!(
            "terminal-{}",
            self.next_id.fetch_add(1, Ordering::Relaxed) + 1
        ));
        self.lock()?.insert(
            id.as_str().to_owned(),
            LiveTerminal {
                master: pair.master,
                child,
                receiver,
                engine: TerminalEngine::new(size, writer),
                pending_events: VecDeque::new(),
                reader_finished,
                exited: false,
            },
        );
        Ok(id)
    }

    fn profiles(&self, _cwd: &Path) -> Result<TerminalProfiles, TerminalError> {
        Ok((*self.profiles).clone())
    }

    fn input(&self, id: &TerminalId, input: &TerminalInput) -> Result<(), TerminalError> {
        let mut terminals = self.lock()?;
        let terminal = terminals
            .get_mut(id.as_str())
            .ok_or_else(|| TerminalError::new("Terminal not found"))?;
        terminal.engine.input(input)
    }

    fn resize(&self, id: &TerminalId, columns: u16, rows: u16) -> Result<(), TerminalError> {
        self.resize_delta(id, columns, rows).map(|_| ())
    }

    fn resize_delta(
        &self,
        id: &TerminalId,
        columns: u16,
        rows: u16,
    ) -> Result<TerminalDelta, TerminalError> {
        let mut terminals = self.lock()?;
        let terminal = terminals
            .get_mut(id.as_str())
            .ok_or_else(|| TerminalError::new("Terminal not found"))?;
        let size = normalized_size(TerminalSize::new(columns, rows));
        terminal
            .master
            .resize(portable_pty::PtySize {
                rows: size.rows,
                cols: size.columns,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| TerminalError::new(error.to_string()))?;
        let (_, delta) = terminal.engine.resize(size);
        terminal
            .pending_events
            .push_back(TerminalEvent::Delta(delta.clone()));
        Ok(delta)
    }

    fn poll(&self, id: &TerminalId) -> Result<Vec<TerminalEvent>, TerminalError> {
        let mut terminals = self.lock()?;
        let terminal = terminals
            .get_mut(id.as_str())
            .ok_or_else(|| TerminalError::new("Terminal not found"))?;
        let mut events = terminal.pending_events.drain(..).collect::<Vec<_>>();
        for event in terminal.receiver.try_iter() {
            match event {
                RawTerminalEvent::Output(data) if !data.is_empty() => {
                    let delta = terminal.engine.process(&data);
                    events.push(TerminalEvent::Delta(delta));
                }
                RawTerminalEvent::Output(_) => {}
                RawTerminalEvent::Failed(message) => events.push(TerminalEvent::Failed(message)),
            }
        }
        events.extend(terminal.engine.take_failures().map(TerminalEvent::Failed));
        if !terminal.exited
            && terminal.reader_finished.load(Ordering::Acquire)
            && let Some(status) = terminal
                .child
                .try_wait()
                .map_err(|error| TerminalError::new(error.to_string()))?
        {
            terminal.exited = true;
            events.push(TerminalEvent::Exited {
                code: Some(status.exit_code() as i32),
            });
        }
        Ok(events)
    }

    fn terminate(&self, id: &TerminalId) -> Result<(), TerminalError> {
        let mut terminals = self.lock()?;
        let terminal = terminals
            .get_mut(id.as_str())
            .ok_or_else(|| TerminalError::new("Terminal not found"))?;
        terminal
            .child
            .kill()
            .map_err(|error| TerminalError::new(error.to_string()))?;
        terminal.exited = true;
        Ok(())
    }

    fn process_id(&self, id: &TerminalId) -> Option<u32> {
        let terminals = self.lock().ok()?;
        terminals.get(id.as_str())?.child.process_id()
    }

    fn changes(&self) -> Option<futures_mpsc::UnboundedReceiver<()>> {
        self.changes.take()
    }
}

fn shell_directory(cwd: &Path, program: &str) -> Result<PathBuf, TerminalError> {
    #[cfg(windows)]
    if let Some(std::path::Component::Prefix(prefix)) = cwd.components().next() {
        use std::path::Prefix;
        match prefix.kind() {
            Prefix::VerbatimDisk(drive) => {
                let mut path = PathBuf::from(format!("{}:", char::from(drive)));
                path.extend(cwd.components().skip(1));
                return Ok(path);
            }
            Prefix::UNC(_, _) | Prefix::VerbatimUNC(_, _)
                if Path::new(program)
                    .file_stem()
                    .is_some_and(|name| name.eq_ignore_ascii_case("cmd")) =>
            {
                return Err(TerminalError::new(
                    "cmdはUNC作業フォルダを使用できません。PowerShellを選択してください",
                ));
            }
            _ => {}
        }
    }
    let _ = program;
    Ok(cwd.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use terminal::TerminalScreen;

    #[test]
    fn default_profiles_are_platform_specific_and_have_a_default() {
        let profiles = default_profiles();
        assert!(!profiles.profiles.is_empty());
        let default = profiles.default.as_ref().expect("default profile is set");
        assert!(
            profiles
                .profiles
                .iter()
                .any(|profile| &profile.id == default),
            "default profile must be published"
        );
    }

    #[test]
    fn terminal_reader_preserves_vt_utf8_boundaries_and_arbitrary_bytes() {
        struct ChunkedReader {
            chunks: Vec<Vec<u8>>,
        }

        impl Read for ChunkedReader {
            fn read(&mut self, target: &mut [u8]) -> std::io::Result<usize> {
                let Some(chunk) = self.chunks.first_mut() else {
                    return Ok(0);
                };
                let length = chunk.len().min(target.len());
                target[..length].copy_from_slice(&chunk[..length]);
                chunk.drain(..length);
                if chunk.is_empty() {
                    self.chunks.remove(0);
                }
                Ok(length)
            }
        }

        let (sender, receiver) = mpsc::channel();
        read_terminal_stream(
            ChunkedReader {
                chunks: vec![
                    b"\x1b[31mred".to_vec(),
                    b"\x1b[2;4H\x1b[2J".to_vec(),
                    vec![0xe3],
                    vec![0x81, 0x82, 0xff, 0x00],
                ],
            },
            sender,
            || {},
        );

        let mut output = Vec::new();
        for event in receiver {
            if let RawTerminalEvent::Output(data) = event {
                output.extend(data);
            }
        }
        assert_eq!(output, b"\x1b[31mred\x1b[2;4H\x1b[2J\xe3\x81\x82\xff\x00");
    }

    #[test]
    fn reader_notifies_for_each_received_chunk() {
        struct FixedReader {
            data: Vec<u8>,
        }

        impl Read for FixedReader {
            fn read(&mut self, target: &mut [u8]) -> std::io::Result<usize> {
                if self.data.is_empty() {
                    return Ok(0);
                }
                let length = self.data.len().min(target.len());
                target[..length].copy_from_slice(&self.data[..length]);
                self.data.drain(..length);
                Ok(length)
            }
        }

        let (sender, _receiver) = mpsc::channel();
        let notifications = std::cell::Cell::new(0);
        read_terminal_stream(
            FixedReader {
                data: b"echo\n".to_vec(),
            },
            sender,
            || notifications.set(notifications.get() + 1),
        );

        assert_eq!(
            notifications.get(),
            1,
            "受信した chunk ごとに Runtime へ起床通知を送る"
        );
    }

    #[test]
    fn change_receiver_is_only_handed_out_once() {
        let backend = LocalTerminalBackend::default();

        assert!(backend.changes().is_some());
        assert!(
            backend.changes().is_none(),
            "変更通知の receiver は 1 度だけ取得できる"
        );
    }

    #[test]
    fn terminal_pty_accepts_input_and_returns_output() {
        let backend = LocalTerminalBackend::default();
        let mut changes = backend
            .changes()
            .expect("local terminal must publish change notifications");
        let id = backend.start(Path::new("."), 100, 30).unwrap();
        let resize = backend.resize_delta(&id, 120, 32).unwrap();
        let mut screen = TerminalScreen::empty(resize.size);
        screen.apply_delta(&resize).unwrap();
        let command = if cfg!(windows) {
            "Write-Output ('lapis-terminal-' + 'smoke')\r"
        } else {
            "printf 'lapis-terminal-%s\\n' smoke\n"
        };
        backend
            .input(&id, &TerminalInput::Text(command.to_owned()))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut output = String::new();
        let mut sequences = Vec::new();
        while Instant::now() < deadline {
            for event in backend.poll(&id).unwrap() {
                if let TerminalEvent::Delta(delta) = event {
                    sequences.push(delta.revision);
                    screen.apply_delta(&delta).unwrap();
                    output = screen.plain_text();
                }
            }
            if output.contains("lapis-terminal-smoke") {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        let notified = Instant::now() + Duration::from_secs(1);
        loop {
            match changes.try_recv() {
                Ok(()) => break,
                Err(error) if !error.is_empty() => {
                    panic!("terminal change stream closed unexpectedly");
                }
                Err(_) if Instant::now() >= notified => {
                    panic!("terminal output did not notify the runtime");
                }
                Err(_) => thread::sleep(Duration::from_millis(10)),
            }
        }
        backend.terminate(&id).unwrap();
        assert!(
            output.contains("lapis-terminal-smoke"),
            "output: {output:?}"
        );
        assert!(
            sequences.windows(2).all(|pair| pair[0] < pair[1]),
            "output sequences: {sequences:?}"
        );
    }
}
