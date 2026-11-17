//! Host 所有の汎用 PTY。生の出力 bytes を上限付き履歴に保持する。

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    io::{Read, Write},
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
};

use portable_pty::{Child, MasterPty, PtySize as NativeSize};
use process::pty::{
    PtyAttachmentId, PtyError, PtyExit, PtyOutput, PtyOwnerId, PtyService, PtySessionId, PtySize,
    PtySpawn,
};

const MAX_SESSIONS: usize = 64;
const OUTPUT_CAPACITY: usize = 1024 * 1024;
const MAX_TRANSFER: usize = 64 * 1024;
/// 1 session の PTY が確保する画面の上限。UI 固有の寸法には依存しない。
const MAX_PTY_CELLS: u32 = 256 * 1024;

fn valid_size(size: PtySize) -> bool {
    size.columns > 0
        && size.rows > 0
        && u32::from(size.columns) * u32::from(size.rows) <= MAX_PTY_CELLS
}

/// PTY と子プロセスは Host が保持し、接続の detach では終了しない。
pub struct LocalPtyService {
    count: AtomicUsize,
    sessions: Mutex<BTreeMap<PtySessionId, Arc<Session>>>,
}

struct Session {
    owner: PtyOwnerId,
    process: Mutex<Process>,
    output: Arc<(Mutex<Output>, Condvar)>,
    input: mpsc::SyncSender<Vec<u8>>,
    attachments: Mutex<BTreeSet<PtyAttachmentId>>,
}

struct Process {
    master: Option<Box<dyn MasterPty + Send>>,
    child: Box<dyn Child + Send + Sync>,
    exit: Option<PtyExit>,
}

#[derive(Default)]
struct Output {
    bytes: VecDeque<u8>,
    first: u64,
    next: u64,
    done: bool,
    error: Option<String>,
    write_error: Option<String>,
}

impl Output {
    fn append(&mut self, data: &[u8], capacity: usize) {
        for &byte in data {
            if self.bytes.len() == capacity {
                self.bytes.pop_front();
                self.first += 1;
            }
            self.bytes.push_back(byte);
            self.next += 1;
        }
    }

    fn read(&self, from: u64, limit: usize) -> Result<PtyOutput, PtyError> {
        if from > self.next || limit == 0 || limit > MAX_TRANSFER {
            return Err(PtyError("invalid PTY output offset or limit".into()));
        }
        let start = from.max(self.first);
        let count = ((self.next - start) as usize).min(limit);
        let bytes = self
            .bytes
            .iter()
            .skip((start - self.first) as usize)
            .take(count)
            .copied()
            .collect();
        Ok(PtyOutput {
            bytes,
            next: start + count as u64,
            lost_before: (from < self.first).then_some(self.first),
            exited: None,
            read_error: self.error.clone(),
            write_error: self.write_error.clone(),
        })
    }
}

impl Default for LocalPtyService {
    fn default() -> Self {
        Self {
            count: AtomicUsize::new(0),
            sessions: Mutex::new(BTreeMap::new()),
        }
    }
}

fn error(error: impl std::fmt::Display) -> PtyError {
    PtyError(error.to_string())
}

impl LocalPtyService {
    /// 所有者が保持するセッション数。終了済みでも回収されるまでは含む。
    pub fn session_count(&self, owner: &PtyOwnerId) -> Result<usize, PtyError> {
        let sessions = self.sessions.lock().map_err(error)?;
        Ok(sessions
            .values()
            .filter(|session| &session.owner == owner)
            .count())
    }

    /// 拡張の停止時に、その拡張が所有する PTY をまとめて回収する。
    pub fn close_owner(&self, owner: &PtyOwnerId) -> Result<(), PtyError> {
        let removed = {
            let mut sessions = self.sessions.lock().map_err(error)?;
            let ids: Vec<_> = sessions
                .iter()
                .filter(|(_, session)| &session.owner == owner)
                .map(|(id, _)| id.clone())
                .collect();
            let removed: Vec<_> = ids
                .into_iter()
                .filter_map(|id| sessions.remove(&id))
                .collect();
            self.count.fetch_sub(removed.len(), Ordering::AcqRel);
            removed
        };
        let mut first_error = None;
        for session in &removed {
            // ???? read ???????????????????
            let result = (|| -> Result<(), PtyError> {
                session.attachments.lock().map_err(error)?.clear();
                session.output.1.notify_all();
                let mut process = session.process.lock().map_err(error)?;
                if process.refresh_exit()?.is_none()
                    && let Err(problem) = process.child.kill()
                {
                    process.refresh_exit()?;
                    if process.exit.is_none() {
                        return Err(error(problem));
                    }
                }
                Ok(())
            })();
            if let Err(problem) = result {
                first_error.get_or_insert(problem);
            }
        }
        drop(removed);
        first_error.map_or(Ok(()), Err)
    }

    fn session(&self, owner: &PtyOwnerId, id: &PtySessionId) -> Result<Arc<Session>, PtyError> {
        self.sessions
            .lock()
            .map_err(error)?
            .get(id)
            .filter(|session| &session.owner == owner)
            .cloned()
            .ok_or_else(|| PtyError("PTY session not found".into()))
    }
}

impl Session {
    fn require_attachment(
        &self,
        attachment: &PtyAttachmentId,
    ) -> Result<MutexGuard<'_, BTreeSet<PtyAttachmentId>>, PtyError> {
        let attachments = self.attachments.lock().map_err(error)?;
        if attachments.contains(attachment) {
            Ok(attachments)
        } else {
            Err(PtyError("PTY session is not attached".into()))
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if self.exit.is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

impl Process {
    fn refresh_exit(&mut self) -> Result<Option<PtyExit>, PtyError> {
        if self.exit.is_none() {
            self.exit = self.child.try_wait().map_err(error)?.map(|status| PtyExit {
                code: i32::try_from(status.exit_code()).ok(),
            });
            if self.exit.is_some() {
                // Windows ConPTY は master を閉じるまで出力が EOF にならない。
                // 出力 mutex は保持せず、reader が残りの bytes を読み切れるようにする。
                drop(self.master.take());
            }
        }
        Ok(self.exit)
    }
}

impl PtyService for LocalPtyService {
    fn spawn(&self, owner: &PtyOwnerId, command: PtySpawn) -> Result<PtySessionId, PtyError> {
        if owner.0.is_empty()
            || owner.0.len() > 256
            || command.program.is_empty()
            || command.cwd.is_empty()
            || !valid_size(command.size)
        {
            return Err(PtyError("invalid PTY command or size".into()));
        }
        self.count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                (value < MAX_SESSIONS).then_some(value + 1)
            })
            .map_err(|_| PtyError("PTY session limit exceeded".into()))?;
        let mut reservation = Reservation {
            count: &self.count,
            committed: false,
        };
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).map_err(error)?;
        let id = PtySessionId(random.iter().map(|byte| format!("{byte:02x}")).collect());
        let pty = portable_pty::native_pty_system()
            .openpty(NativeSize {
                rows: command.size.rows,
                cols: command.size.columns,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(error)?;
        let mut writer = pty.master.take_writer().map_err(error)?;
        let mut reader = pty.master.try_clone_reader().map_err(error)?;
        let mut builder = portable_pty::CommandBuilder::new(&command.program);
        #[cfg(windows)]
        builder.set_owned_process_tree(true);
        for arg in &command.args {
            builder.arg(arg);
        }
        for (key, value) in &command.env {
            builder.env(key, value);
        }
        builder.cwd(&command.cwd);
        let child = pty.slave.spawn_command(builder).map_err(error)?;
        drop(pty.slave);
        let process = Process {
            master: Some(pty.master),
            child,
            exit: None,
        };
        let output = Arc::new((Mutex::new(Output::default()), Condvar::new()));
        let reader_output = Arc::clone(&output);
        thread::Builder::new()
            .name("lapis-pty-reader".into())
            .spawn(move || {
                let mut buffer = [0; 4096];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(n) => match reader_output.0.lock() {
                            Ok(mut output) => {
                                output.append(&buffer[..n], OUTPUT_CAPACITY);
                                reader_output.1.notify_all();
                            }
                            Err(_) => return,
                        },
                        Err(e)
                            if matches!(
                                e.kind(),
                                std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::UnexpectedEof
                            ) =>
                        {
                            break;
                        }
                        Err(e) => {
                            if let Ok(mut output) = reader_output.0.lock() {
                                output.error = Some(e.to_string());
                            }
                            break;
                        }
                    }
                }
                if let Ok(mut output) = reader_output.0.lock() {
                    output.done = true;
                    reader_output.1.notify_all();
                }
            })
            .map_err(error)?;
        let (input, receiver) = mpsc::sync_channel::<Vec<u8>>(16);
        let writer_output = Arc::clone(&output);
        thread::Builder::new()
            .name("lapis-pty-writer".into())
            .spawn(move || {
                for bytes in receiver {
                    if let Err(error) = writer.write_all(&bytes).and_then(|_| writer.flush()) {
                        if let Ok(mut output) = writer_output.0.lock() {
                            output.write_error = Some(error.to_string());
                            writer_output.1.notify_all();
                        }
                        break;
                    }
                }
            })
            .map_err(error)?;
        let session = Arc::new(Session {
            owner: owner.clone(),
            process: Mutex::new(process),
            output,
            input,
            attachments: Mutex::new(BTreeSet::new()),
        });
        let mut sessions = self.sessions.lock().map_err(error)?;
        if sessions.contains_key(&id) {
            return Err(PtyError("PTY identifier collision".into()));
        }
        sessions.insert(id.clone(), session);
        reservation.committed = true;
        Ok(id)
    }

    fn attach(
        &self,
        owner: &PtyOwnerId,
        id: &PtySessionId,
        attachment: PtyAttachmentId,
    ) -> Result<(), PtyError> {
        if attachment.0.is_empty() || attachment.0.len() > 256 {
            return Err(PtyError("invalid PTY attachment".into()));
        }
        let sessions = self.sessions.lock().map_err(error)?;
        let session = sessions
            .get(id)
            .filter(|session| &session.owner == owner)
            .ok_or_else(|| PtyError("PTY session not found".into()))?;
        session
            .attachments
            .lock()
            .map_err(error)?
            .insert(attachment);
        Ok(())
    }

    fn detach(
        &self,
        owner: &PtyOwnerId,
        id: &PtySessionId,
        attachment: &PtyAttachmentId,
    ) -> Result<(), PtyError> {
        let sessions = self.sessions.lock().map_err(error)?;
        let session = sessions
            .get(id)
            .filter(|session| &session.owner == owner)
            .ok_or_else(|| PtyError("PTY session not found".into()))?;
        session
            .attachments
            .lock()
            .map_err(error)?
            .remove(attachment);
        session.output.1.notify_all();
        Ok(())
    }

    fn write(
        &self,
        owner: &PtyOwnerId,
        id: &PtySessionId,
        attachment: &PtyAttachmentId,
        data: &[u8],
    ) -> Result<(), PtyError> {
        if data.len() > MAX_TRANSFER {
            return Err(PtyError("PTY input limit exceeded".into()));
        }
        let session = self.session(owner, id)?;
        let _attachment = session.require_attachment(attachment)?;
        session.input.try_send(data.to_vec()).map_err(|error| {
            PtyError(
                match error {
                    mpsc::TrySendError::Full(_) => "PTY input queue is full",
                    mpsc::TrySendError::Disconnected(_) => "PTY input is closed",
                }
                .into(),
            )
        })
    }

    fn resize(
        &self,
        owner: &PtyOwnerId,
        id: &PtySessionId,
        attachment: &PtyAttachmentId,
        size: PtySize,
    ) -> Result<(), PtyError> {
        if !valid_size(size) {
            return Err(PtyError("invalid PTY size".into()));
        }
        let session = self.session(owner, id)?;
        let _attachment = session.require_attachment(attachment)?;
        session
            .process
            .lock()
            .map_err(error)?
            .master
            .as_ref()
            .ok_or_else(|| PtyError("PTY session has exited".into()))?
            .resize(NativeSize {
                rows: size.rows,
                cols: size.columns,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(error)
    }

    fn read(
        &self,
        owner: &PtyOwnerId,
        id: &PtySessionId,
        attachment: &PtyAttachmentId,
        from: u64,
        limit: usize,
    ) -> Result<PtyOutput, PtyError> {
        let session = self.session(owner, id)?;
        let _attachment = session.require_attachment(attachment)?;
        let exit = session.process.lock().map_err(error)?.refresh_exit()?;
        let output = session.output.0.lock().map_err(error)?;
        let mut result = output.read(from, limit)?;
        if output.done && result.next == output.next {
            result.exited = exit;
        }
        Ok(result)
    }

    fn wait_read(
        &self,
        owner: &PtyOwnerId,
        id: &PtySessionId,
        attachment: &PtyAttachmentId,
        from: u64,
        limit: usize,
        timeout_ms: u64,
    ) -> Result<PtyOutput, PtyError> {
        if limit == 0 || limit > MAX_TRANSFER || timeout_ms > 30_000 {
            return Err(PtyError("invalid PTY output limit or timeout".into()));
        }
        let session = self.session(owner, id)?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        loop {
            drop(session.require_attachment(attachment)?);
            session.process.lock().map_err(error)?.refresh_exit()?;
            let output = session.output.0.lock().map_err(error)?;
            if from > output.next {
                return Err(PtyError("invalid PTY output offset".into()));
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if output.next != from
                || output.done
                || output.write_error.is_some()
                || remaining.is_zero()
            {
                break;
            }
            // detach を確認できるよう、待機中は attachment lock を保持しない。
            let (output, _) = session
                .output
                .1
                .wait_timeout(output, remaining.min(std::time::Duration::from_millis(100)))
                .map_err(error)?;
            drop(output);
        }
        self.read(owner, id, attachment, from, limit)
    }

    fn kill(
        &self,
        owner: &PtyOwnerId,
        id: &PtySessionId,
        attachment: &PtyAttachmentId,
    ) -> Result<(), PtyError> {
        let session = self.session(owner, id)?;
        let _attachment = session.require_attachment(attachment)?;
        let mut process = session.process.lock().map_err(error)?;
        if process.refresh_exit()?.is_none()
            && let Err(problem) = process.child.kill()
        {
            process.refresh_exit()?;
            if process.exit.is_none() {
                return Err(error(problem));
            }
        }
        Ok(())
    }

    fn forget(&self, owner: &PtyOwnerId, id: &PtySessionId) -> Result<(), PtyError> {
        let mut sessions = self.sessions.lock().map_err(error)?;
        let session = sessions
            .get(id)
            .filter(|session| &session.owner == owner)
            .ok_or_else(|| PtyError("PTY session not found".into()))?;
        let attachments = session.attachments.lock().map_err(error)?;
        if !attachments.is_empty() {
            return Err(PtyError("PTY session is still attached".into()));
        }
        let mut process = session.process.lock().map_err(error)?;
        if process.refresh_exit()?.is_none() {
            return Err(PtyError("PTY session is still running".into()));
        }
        drop(process);
        let output = session.output.0.lock().map_err(error)?;
        if !output.done {
            return Err(PtyError("PTY output is still open".into()));
        }
        drop(output);
        drop(attachments);
        sessions.remove(id);
        self.count.fetch_sub(1, Ordering::AcqRel);
        Ok(())
    }
}

struct Reservation<'a> {
    count: &'a AtomicUsize,
    committed: bool,
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        if !self.committed {
            self.count.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_limit_exposes_lost_bytes_and_ordered_offsets() {
        let mut output = Output::default();
        output.append(b"abc", 5);
        output.append(b"defg", 5);
        let first = output.read(0, 2).unwrap();
        assert_eq!(first.bytes, b"cd");
        assert_eq!(first.lost_before, Some(2));
        assert_eq!(first.next, 4);
        let rest = output.read(first.next, 5).unwrap();
        assert_eq!(rest.bytes, b"efg");
        assert_eq!(rest.next, 7);
        assert_eq!(rest.lost_before, None);
        assert!(output.read(8, 1).is_err());
        assert!(output.read(7, 0).is_err());
    }

    fn command() -> PtySpawn {
        let (program, args): (&str, Vec<&str>) = if cfg!(windows) {
            (
                "cmd.exe",
                vec![
                    "/D",
                    "/Q",
                    "/C",
                    "echo pty-ready & set /p x= & echo pty-done",
                ],
            )
        } else {
            (
                "sh",
                vec!["-c", "printf 'pty-ready\\n'; read x; printf 'pty-done\\n'"],
            )
        };
        PtySpawn {
            program: program.to_owned(),
            args: args.into_iter().map(str::to_owned).collect(),
            env: BTreeMap::new(),
            cwd: std::env::current_dir()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            size: PtySize {
                columns: 80,
                rows: 24,
            },
        }
    }

    fn wait_for(
        service: &LocalPtyService,
        owner: &PtyOwnerId,
        id: &PtySessionId,
        attachment: &PtyAttachmentId,
        mut cursor: u64,
        needle: &[u8],
    ) -> (u64, bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        let mut accumulated = Vec::new();
        let mut answered_cursor_query = false;
        while std::time::Instant::now() < deadline {
            let result = service
                .wait_read(owner, id, attachment, cursor, 4096, 200)
                .unwrap();
            assert!(result.lost_before.is_none());
            assert!(result.read_error.is_none(), "{:?}", result.read_error);
            assert!(result.write_error.is_none(), "{:?}", result.write_error);
            accumulated.extend_from_slice(&result.bytes);
            cursor = result.next;
            // Windows ConPTY は初期化時に DSR を発行する。解釈・応答は利用側の責務。
            if !answered_cursor_query && accumulated.windows(4).any(|bytes| bytes == b"\x1b[6n") {
                service.write(owner, id, attachment, b"\x1b[1;1R").unwrap();
                answered_cursor_query = true;
            }
            if accumulated
                .windows(needle.len())
                .any(|bytes| bytes == needle)
            {
                return (cursor, result.exited.is_some());
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!(
            "expected {:?} in {:?}",
            String::from_utf8_lossy(needle),
            String::from_utf8_lossy(&accumulated)
        );
    }

    #[test]
    fn process_survives_detach_and_is_isolated_by_owner() {
        let service = LocalPtyService::default();
        let owner = PtyOwnerId("extension.a".into());
        let other = PtyOwnerId("extension.b".into());
        let first = PtyAttachmentId("client-1".into());
        let second = PtyAttachmentId("client-2".into());
        let id = service.spawn(&owner, command()).unwrap();
        assert!(service.attach(&other, &id, first.clone()).is_err());
        service.attach(&owner, &id, first.clone()).unwrap();
        let (cursor, _) = wait_for(&service, &owner, &id, &first, 0, b"pty-ready");
        service.detach(&owner, &id, &first).unwrap();
        assert!(service.write(&owner, &id, &first, b"denied\r\n").is_err());
        service.attach(&owner, &id, second.clone()).unwrap();
        assert!(service.forget(&owner, &id).is_err());
        service
            .resize(
                &owner,
                &id,
                &second,
                PtySize {
                    columns: 100,
                    rows: 30,
                },
            )
            .unwrap();
        service.write(&owner, &id, &second, b"hello\r\n").unwrap();
        let (cursor, exited) = wait_for(&service, &owner, &id, &second, cursor, b"pty-done");
        if !exited {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
            let mut position = cursor;
            loop {
                let result = service
                    .wait_read(&owner, &id, &second, position, 4096, 200)
                    .unwrap();
                position = result.next;
                if result.exited.is_some() {
                    break;
                }
                assert!(std::time::Instant::now() < deadline, "child did not exit");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        service.detach(&owner, &id, &second).unwrap();
        assert!(service.forget(&other, &id).is_err());
        service.forget(&owner, &id).unwrap();
        assert!(service.attach(&owner, &id, second).is_err());
    }

    #[test]
    fn closing_owner_stops_process_even_when_session_is_referenced() {
        let service = LocalPtyService::default();
        let owner = PtyOwnerId("extension.a".into());
        let attachment = PtyAttachmentId("client".into());
        let id = service.spawn(&owner, command()).unwrap();
        service.attach(&owner, &id, attachment.clone()).unwrap();
        wait_for(&service, &owner, &id, &attachment, 0, b"pty-ready");
        // ????????????????????????????
        let session = service.session(&owner, &id).unwrap();
        service.close_owner(&owner).unwrap();
        assert_eq!(service.session_count(&owner).unwrap(), 0);
        assert!(session.require_attachment(&attachment).is_err());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if session
                .process
                .lock()
                .unwrap()
                .refresh_exit()
                .unwrap()
                .is_some()
            {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "child did not exit");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn invalid_pty_area_is_rejected_before_start_or_resize() {
        let service = LocalPtyService::default();
        let owner = PtyOwnerId("extension.a".into());
        let mut oversized = command();
        oversized.size = PtySize {
            columns: 4096,
            rows: 4096,
        };
        assert!(service.spawn(&owner, oversized).is_err());
        assert_eq!(service.session_count(&owner).unwrap(), 0);
        let error = service
            .resize(
                &owner,
                &PtySessionId("missing".into()),
                &PtyAttachmentId("client".into()),
                PtySize {
                    columns: 4096,
                    rows: 4096,
                },
            )
            .unwrap_err();
        assert_eq!(error.0, "invalid PTY size");
        assert!(valid_size(PtySize {
            columns: 1024,
            rows: 256
        }));
        assert!(!valid_size(PtySize {
            columns: 1025,
            rows: 256
        }));
    }

    #[test]
    fn stopping_an_extension_reclaims_only_its_pty_sessions() {
        let service = LocalPtyService::default();
        let first = PtyOwnerId("extension.first".into());
        let second = PtyOwnerId("extension.second".into());
        let attachment = PtyAttachmentId("client".into());
        let first_id = service.spawn(&first, command()).unwrap();
        let second_id = service.spawn(&second, command()).unwrap();
        service
            .attach(&first, &first_id, attachment.clone())
            .unwrap();
        service
            .attach(&second, &second_id, attachment.clone())
            .unwrap();
        wait_for(&service, &first, &first_id, &attachment, 0, b"pty-ready");
        let (cursor, _) = wait_for(&service, &second, &second_id, &attachment, 0, b"pty-ready");
        service.close_owner(&first).unwrap();
        assert!(
            service
                .write(&first, &first_id, &attachment, b"ignored")
                .is_err()
        );
        service
            .write(&second, &second_id, &attachment, b"continue\r\n")
            .unwrap();
        wait_for(
            &service,
            &second,
            &second_id,
            &attachment,
            cursor,
            b"pty-done",
        );
        service.close_owner(&second).unwrap();
    }

    #[test]
    fn kill_reports_exit_after_remaining_output_and_allows_cleanup() {
        let service = LocalPtyService::default();
        let owner = PtyOwnerId("extension.a".into());
        let attachment = PtyAttachmentId("client".into());
        let id = service.spawn(&owner, command()).unwrap();
        service.attach(&owner, &id, attachment.clone()).unwrap();
        let (mut cursor, _) = wait_for(&service, &owner, &id, &attachment, 0, b"pty-ready");
        service.kill(&owner, &id, &attachment).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            let result = service
                .wait_read(&owner, &id, &attachment, cursor, 4096, 200)
                .unwrap();
            cursor = result.next;
            if result.exited.is_some() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "killed child did not exit"
            );
        }
        service.detach(&owner, &id, &attachment).unwrap();
        service.forget(&owner, &id).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn killing_pty_terminates_the_spawned_process_tree() {
        use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
        };

        let directory = tempfile::tempdir().unwrap();
        let pid_file = directory.path().join("child.pid");
        let path = pid_file.to_string_lossy().replace('\'', "''");
        let script = format!(
            "$p=Start-Process -FilePath cmd.exe -ArgumentList '/D','/C','ping -n 120 127.0.0.1 >nul' -WindowStyle Hidden -PassThru; [IO.File]::WriteAllText('{path}', $p.Id.ToString()); Write-Output 'child-ready'; Start-Sleep -Seconds 120"
        );
        let mut spawn = command();
        spawn.program = "powershell.exe".into();
        spawn.args = vec!["-NoProfile".into(), "-Command".into(), script];
        let service = LocalPtyService::default();
        let owner = PtyOwnerId("extension.example".into());
        let attachment = PtyAttachmentId("client".into());
        let id = service.spawn(&owner, spawn).unwrap();
        service.attach(&owner, &id, attachment.clone()).unwrap();
        wait_for(&service, &owner, &id, &attachment, 0, b"child-ready");
        let pid: u32 = std::fs::read_to_string(pid_file).unwrap().parse().unwrap();
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        assert!(!handle.is_null(), "child process must exist");
        assert_eq!(unsafe { WaitForSingleObject(handle, 0) }, WAIT_TIMEOUT);
        let result = service.kill(&owner, &id, &attachment);
        let exited = unsafe { WaitForSingleObject(handle, 10_000) };
        unsafe { CloseHandle(handle) };
        result.unwrap();
        assert_eq!(
            exited, WAIT_OBJECT_0,
            "child process survived PTY termination"
        );
    }

    #[cfg(windows)]
    #[test]
    fn crashed_host_fixture() {
        let Some(directory) = std::env::var_os("LAPIS_PTY_TEST_DIRECTORY") else {
            return;
        };
        let directory = std::path::PathBuf::from(directory);
        let path = directory
            .join("child.pid")
            .to_string_lossy()
            .replace('\'', "''");
        let script = format!(
            "$p=Start-Process -FilePath cmd.exe -ArgumentList '/D','/C','ping -n 120 127.0.0.1 >nul' -WindowStyle Hidden -PassThru; [IO.File]::WriteAllText('{path}', $p.Id.ToString()); Write-Output 'child-ready'; Start-Sleep -Seconds 120"
        );
        let mut spawn = command();
        spawn.program = "powershell.exe".into();
        spawn.args = vec!["-NoProfile".into(), "-Command".into(), script];
        let service = LocalPtyService::default();
        let owner = PtyOwnerId("extension.example".into());
        let attachment = PtyAttachmentId("client".into());
        let id = service.spawn(&owner, spawn).unwrap();
        service.attach(&owner, &id, attachment.clone()).unwrap();
        wait_for(&service, &owner, &id, &attachment, 0, b"child-ready");
        std::fs::write(directory.join("ready"), b"ready").unwrap();
        std::thread::sleep(std::time::Duration::from_secs(120));
    }

    #[cfg(windows)]
    #[test]
    fn host_crash_terminates_pty_process_tree() {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
        };

        struct KillOnDrop(std::process::Child);
        impl Drop for KillOnDrop {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let mut host = KillOnDrop(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "pty::tests::crashed_host_fixture"])
                .env("LAPIS_PTY_TEST_DIRECTORY", directory.path())
                .creation_flags(0x08000000)
                .spawn()
                .unwrap(),
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !directory.path().join("ready").exists() {
            assert!(
                host.0.try_wait().unwrap().is_none(),
                "fixture exited before starting PTY"
            );
            assert!(
                std::time::Instant::now() < deadline,
                "fixture did not start PTY"
            );
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        let pid: u32 = std::fs::read_to_string(directory.path().join("child.pid"))
            .unwrap()
            .parse()
            .unwrap();
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        assert!(!handle.is_null());
        assert_eq!(unsafe { WaitForSingleObject(handle, 0) }, WAIT_TIMEOUT);
        host.0.kill().unwrap();
        host.0.wait().unwrap();
        let exited = unsafe { WaitForSingleObject(handle, 10_000) };
        unsafe { CloseHandle(handle) };
        assert_eq!(exited, WAIT_OBJECT_0, "PTY descendant survived Host crash");
    }
}
