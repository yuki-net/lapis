//! Host 所有の PTY を `TerminalHost` port へ接続する。
//!
//! PTY の生成・読み書き・終了は `local-process::pty::LocalPtyService` に任せ、
//! この adapter は port の文字列 ID と所有権・attachment の対応だけを持つ。

use std::sync::Arc;

use local_process::pty::LocalPtyService;
use process::pty::{PtyAttachmentId, PtyOwnerId, PtyService, PtySessionId, PtySize, PtySpawn};
use terminal_host::{TerminalHost, process_info, sessions};

/// Host が所有する PTY の attachment。1 つの Host で共有する。
const ATTACHMENT: &str = "terminal-host";

/// `TerminalHost` port の Host 実装。
#[derive(Clone)]
pub struct LocalTerminalHost {
    pty: Arc<LocalPtyService>,
    owner: PtyOwnerId,
    attachment: PtyAttachmentId,
    program: String,
    arguments: Vec<String>,
}

impl LocalTerminalHost {
    /// 既定の shell は OS の標準 shell。`with_shell` でテスト用に差し替えられる。
    pub fn new(pty: Arc<LocalPtyService>) -> Self {
        #[cfg(windows)]
        let (program, arguments) = ("powershell.exe".to_owned(), vec!["-NoLogo".to_owned()]);
        #[cfg(not(windows))]
        let (program, arguments) = (
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned()),
            Vec::new(),
        );
        Self::with_shell(pty, program, arguments)
    }

    pub fn with_shell(
        pty: Arc<LocalPtyService>,
        program: impl Into<String>,
        arguments: Vec<String>,
    ) -> Self {
        Self {
            pty,
            owner: PtyOwnerId("terminal-host".to_owned()),
            attachment: PtyAttachmentId(ATTACHMENT.to_owned()),
            program: program.into(),
            arguments,
        }
    }

    fn session(&self, id: &str) -> PtySessionId {
        PtySessionId(id.to_owned())
    }
}

/// Windows の verbatim prefix は shell の作業ディレクトリとして使えないため外す。
fn native_directory(cwd: &str) -> String {
    #[cfg(windows)]
    {
        use std::path::{Component, Path, Prefix};
        let path = Path::new(cwd);
        if let Some(Component::Prefix(prefix)) = path.components().next()
            && let Prefix::VerbatimDisk(drive) = prefix.kind()
        {
            let mut native = std::path::PathBuf::from(format!("{}:", char::from(drive)));
            native.extend(path.components().skip(1));
            return native
                .to_str()
                .map(str::to_owned)
                .unwrap_or_else(|| cwd.to_owned());
        }
    }
    cwd.to_owned()
}

impl TerminalHost for LocalTerminalHost {
    fn information(&self) -> Result<process_info::Information, String> {
        Ok(process_info::Information {
            #[cfg(windows)]
            pty: process_info::PtyFlavor::Conpty,
            #[cfg(not(windows))]
            pty: process_info::PtyFlavor::Posix,
            default_shell: process_info::Command {
                program: self.program.clone(),
                arguments: self.arguments.clone(),
            },
        })
    }

    fn spawn(&self, command: &sessions::SpawnRequest) -> Result<String, String> {
        let spawn = PtySpawn {
            program: command.program.clone(),
            args: command.arguments.clone(),
            env: command
                .environment
                .iter()
                .map(|entry| (entry.name.clone(), entry.value.clone()))
                .collect(),
            cwd: native_directory(&command.working_directory),
            size: PtySize {
                columns: command.initial_size.columns,
                rows: command.initial_size.rows,
            },
        };
        self.pty
            .spawn(&self.owner, spawn)
            .map(|id| id.0)
            .map_err(|error| error.to_string())
    }

    fn attach(&self, id: &str) -> Result<(), String> {
        self.pty
            .attach(&self.owner, &self.session(id), self.attachment.clone())
            .map_err(|error| error.to_string())
    }

    fn detach(&self, id: &str) -> Result<(), String> {
        self.pty
            .detach(&self.owner, &self.session(id), &self.attachment)
            .map_err(|error| error.to_string())
    }

    fn write(&self, id: &str, bytes: &[u8]) -> Result<(), String> {
        self.pty
            .write(&self.owner, &self.session(id), &self.attachment, bytes)
            .map_err(|error| error.to_string())
    }

    fn resize(&self, id: &str, size: sessions::Size) -> Result<(), String> {
        self.pty
            .resize(
                &self.owner,
                &self.session(id),
                &self.attachment,
                PtySize {
                    columns: size.columns,
                    rows: size.rows,
                },
            )
            .map_err(|error| error.to_string())
    }

    fn read(
        &self,
        id: &str,
        offset: u64,
        max_bytes: u32,
        timeout_ms: u32,
    ) -> Result<sessions::Output, String> {
        let id = self.session(id);
        if timeout_ms > 0 {
            self.pty
                .wait_read(
                    &self.owner,
                    &id,
                    &self.attachment,
                    offset,
                    max_bytes as usize,
                    u64::from(timeout_ms),
                )
                .map_err(|error| error.to_string())?;
        }
        let output = self
            .pty
            .read(
                &self.owner,
                &id,
                &self.attachment,
                offset,
                max_bytes as usize,
            )
            .map_err(|error| error.to_string())?;
        Ok(sessions::Output {
            bytes: output.bytes,
            next: output.next,
            lost_before: output.lost_before,
            exit: output
                .exited
                .map(|exit| sessions::ExitStatus { code: exit.code }),
            read_error: output.read_error,
            write_error: output.write_error,
        })
    }

    fn kill(&self, id: &str) -> Result<(), String> {
        self.pty
            .kill(&self.owner, &self.session(id), &self.attachment)
            .map_err(|error| error.to_string())
    }

    fn forget(&self, id: &str) -> Result<(), String> {
        self.pty
            .forget(&self.owner, &self.session(id))
            .map_err(|error| error.to_string())
    }
}
