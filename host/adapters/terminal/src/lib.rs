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

use terminal::{
    TerminalBackend, TerminalDelta, TerminalError, TerminalEvent, TerminalId, TerminalInput,
    TerminalSize,
};

mod terminal_engine;
use terminal_engine::{TerminalEngine, normalized_size};

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

#[derive(Clone, Default)]
pub struct LocalTerminalBackend {
    terminals: Arc<Mutex<HashMap<String, LiveTerminal>>>,
    next_id: Arc<AtomicU64>,
}

impl LocalTerminalBackend {
    fn lock(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, HashMap<String, LiveTerminal>>, TerminalError> {
        self.terminals
            .lock()
            .map_err(|_| TerminalError::new("Terminal state lock failed"))
    }
}

fn read_terminal_stream<R: Read>(mut reader: R, sender: mpsc::Sender<RawTerminalEvent>) {
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
        self.start_with_shell(cwd, columns, rows, terminal::TerminalShell::Default)
    }

    fn start_with_shell(
        &self,
        cwd: &Path,
        columns: u16,
        rows: u16,
        shell: terminal::TerminalShell,
    ) -> Result<TerminalId, TerminalError> {
        use terminal::TerminalShell;
        let size = normalized_size(TerminalSize::new(columns, rows));
        let cwd = shell_directory(cwd, shell)?;
        let mut command = match shell {
            #[cfg(windows)]
            TerminalShell::Default | TerminalShell::PowerShell => {
                let mut command = portable_pty::CommandBuilder::new("powershell.exe");
                command.arg("-NoLogo");
                command
            }
            #[cfg(windows)]
            TerminalShell::Cmd => {
                let mut command = portable_pty::CommandBuilder::new("cmd.exe");
                command.arg("/D");
                command
            }
            #[cfg(windows)]
            TerminalShell::Bash | TerminalShell::Zsh | TerminalShell::Fish => {
                return Err(TerminalError::new(
                    "bash / zsh / fish は Windows Host では利用できません",
                ));
            }
            #[cfg(not(windows))]
            TerminalShell::Default => portable_pty::CommandBuilder::new(
                std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned()),
            ),
            #[cfg(not(windows))]
            TerminalShell::PowerShell => portable_pty::CommandBuilder::new("pwsh"),
            #[cfg(not(windows))]
            TerminalShell::Bash => portable_pty::CommandBuilder::new("bash"),
            #[cfg(not(windows))]
            TerminalShell::Zsh => portable_pty::CommandBuilder::new("zsh"),
            #[cfg(not(windows))]
            TerminalShell::Fish => portable_pty::CommandBuilder::new("fish"),
            #[cfg(not(windows))]
            TerminalShell::Cmd => {
                return Err(TerminalError::new("cmdはWindows Hostでのみ利用できます"));
            }
        };
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
        thread::spawn(move || {
            read_terminal_stream(reader, sender);
            reader_finished_for_thread.store(true, Ordering::Release);
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
}

fn shell_directory(cwd: &Path, shell: terminal::TerminalShell) -> Result<PathBuf, TerminalError> {
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
                if shell == terminal::TerminalShell::Cmd =>
            {
                return Err(TerminalError::new(
                    "cmdはUNC作業フォルダを使用できません。PowerShellを選択してください",
                ));
            }
            _ => {}
        }
    }
    let _ = shell;
    Ok(cwd.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use terminal::TerminalScreen;

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
    fn terminal_pty_accepts_input_and_returns_output() {
        let backend = LocalTerminalBackend::default();
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
