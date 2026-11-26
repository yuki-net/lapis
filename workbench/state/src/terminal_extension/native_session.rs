//! 拡張の PTY と Desktop 描画器をつなぐ同期セッション。呼び出しは worker で行う。

use std::{
    io::{self, Write},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

use client_api::{HostExtensionDispatchRequest, HostExtensionDispatchResponse};
use serde::{Serialize, de::DeserializeOwned};
use terminal::{TerminalDelta, TerminalInput, TerminalScreen, TerminalSize};

use super::{NativeOutput, NativeRenderer, request, response, v1};

/// Host 操作を行う transport。呼び出し元が接続と認可を所有する。
pub type NativeTransport = Arc<
    dyn Fn(HostExtensionDispatchRequest) -> Result<HostExtensionDispatchResponse, String>
        + Send
        + Sync,
>;

fn call<T: Serialize, R: DeserializeOwned>(
    transport: &NativeTransport,
    operation: &str,
    value: &T,
) -> Result<R, String> {
    let input = request(operation, value)?;
    response(operation, transport(input)?)
}

struct InputWriter {
    sender: mpsc::SyncSender<Vec<u8>>,
    failure: Arc<Mutex<Option<String>>>,
}

impl InputWriter {
    fn new(id: String, transport: NativeTransport) -> Self {
        let (sender, receiver) = mpsc::sync_channel::<Vec<u8>>(128);
        let failure = Arc::new(Mutex::new(None));
        let failure_state = failure.clone();
        std::thread::spawn(move || {
            let mut pending = None;
            while let Some(mut bytes) = pending.take().or_else(|| receiver.recv().ok()) {
                let deadline = Instant::now() + Duration::from_millis(2);
                while bytes.len() < v1::MAX_READ_BYTES as usize {
                    let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                        break;
                    };
                    match receiver.recv_timeout(remaining) {
                        Ok(next) if bytes.len() + next.len() <= v1::MAX_READ_BYTES as usize => {
                            bytes.extend(next)
                        }
                        Ok(next) => {
                            pending = Some(next);
                            break;
                        }
                        Err(_) => break,
                    }
                }
                let request = v1::WriteRequest {
                    id: id.clone(),
                    bytes,
                };
                if let Err(error) = call::<_, ()>(&transport, v1::WRITE, &request) {
                    if let Ok(mut failure) = failure_state.lock() {
                        *failure = Some(error);
                    }
                    break;
                }
            }
        });
        Self { sender, failure }
    }
}

impl Write for InputWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        // byte 配列を JSON にした際にも汎用 transport の上限を超えない。
        let count = bytes.len().min(v1::MAX_READ_BYTES as usize);
        if count == 0 {
            return Ok(0);
        }
        if let Some(error) = self
            .failure
            .lock()
            .map_err(|_| io::Error::other("Terminal input state unavailable"))?
            .as_ref()
        {
            return Err(io::Error::other(error.clone()));
        }
        self.sender
            .send(bytes[..count].to_vec())
            .map_err(io::Error::other)?;
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 現在の Client の描画状態。Drop は Host の PTY を終了せず、明示的な close のみが終了する。
pub struct NativeSession {
    id: String,
    working_directory: String,
    transport: NativeTransport,
    renderer: NativeRenderer,
    write_failure: Arc<Mutex<Option<String>>>,
    closed: bool,
    owner: Option<String>,
}

impl NativeSession {
    /// Host の既定シェルを使う。ユーザー設定の profile があれば shell に渡す。
    pub fn start(
        transport: NativeTransport,
        cwd: String,
        size: TerminalSize,
        shell: Option<v1::ShellCommand>,
    ) -> Result<Self, String> {
        let size = checked_size(size)?;
        let information: v1::InfoResponse = call(&transport, v1::INFO, &v1::InfoRequest {})?;
        let shell = shell.unwrap_or(information.default_shell);
        let started: v1::StartResponse = call(
            &transport,
            v1::START,
            &v1::StartRequest {
                requested_id: None,
                program: shell.program,
                arguments: shell.arguments,
                environment: shell.environment,
                working_directory: cwd.clone(),
                columns: size.columns,
                rows: size.rows,
            },
        )?;
        Self::from_started(transport, cwd, size, information.pty, started)
    }

    /// 拡張が管理する profile を ID で選ぶ。実行する command は拡張側で決める。
    pub fn start_profile(
        transport: NativeTransport,
        cwd: String,
        size: TerminalSize,
        profile: Option<String>,
    ) -> Result<Self, String> {
        let size = checked_size(size)?;
        let information: v1::InfoResponse = call(&transport, v1::INFO, &v1::InfoRequest {})?;
        let started = call(
            &transport,
            v1::START_PROFILE,
            &v1::StartProfileRequest {
                requested_id: None,
                profile,
                working_directory: cwd.clone(),
                columns: size.columns,
                rows: size.rows,
            },
        )?;
        Self::from_started(transport, cwd, size, information.pty, started)
    }

    fn from_started(
        transport: NativeTransport,
        cwd: String,
        size: TerminalSize,
        pty: v1::PtyFlavor,
        started: v1::StartResponse,
    ) -> Result<Self, String> {
        if started.id.is_empty() || started.id.len() > 256 {
            return Err("Terminal 拡張の session ID が不正です".into());
        }
        let writer = InputWriter::new(started.id.clone(), transport.clone());
        let write_failure = writer.failure.clone();
        let renderer = NativeRenderer::new(started.id.clone(), size, Box::new(writer), pty);
        Ok(Self {
            id: started.id,
            working_directory: cwd,
            transport,
            renderer,
            write_failure,
            closed: false,
            owner: started.owner,
        })
    }

    pub fn profiles(transport: &NativeTransport) -> Result<terminal::TerminalProfiles, String> {
        let profiles: v1::ProfilesResponse =
            call(transport, v1::PROFILES, &v1::ProfilesRequest {})?;
        super::profiles_from_wire(profiles)
    }

    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn working_directory(&self) -> &str {
        &self.working_directory
    }

    pub fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }

    /// 拡張側の生存セッション。Host の Workspace や権限は transport 側が管理する。
    pub fn list(transport: &NativeTransport) -> Result<Vec<v1::Session>, String> {
        let list: v1::ListResponse = call(transport, v1::LIST, &v1::ListRequest {})?;
        let mut ids = std::collections::BTreeSet::new();
        if list.sessions.len() > 64
            || list.sessions.iter().any(|session| {
                session.id.is_empty()
                    || session.id.len() > 256
                    || session.columns == 0
                    || session.columns > 512
                    || session.rows == 0
                    || session.rows > 256
                    || !ids.insert(session.id.clone())
            })
        {
            return Err("Terminal 拡張の session 一覧が不正です".into());
        }
        Ok(list.sessions)
    }

    /// 既存 PTY は残したまま、出力を先頭から読み直せる描画状態を作る。
    pub fn attach(transport: NativeTransport, id: &str) -> Result<Self, String> {
        let session = Self::list(&transport)?
            .into_iter()
            .find(|session| session.id == id)
            .ok_or("Terminal session が見つかりません")?;
        let information: v1::InfoResponse = call(&transport, v1::INFO, &v1::InfoRequest {})?;
        let attached: v1::OwnerResponse = call(
            &transport,
            v1::ATTACH,
            &v1::IdRequest {
                id: session.id.clone(),
            },
        )?;
        let writer = InputWriter::new(session.id.clone(), transport.clone());
        let write_failure = writer.failure.clone();
        let renderer = NativeRenderer::new(
            session.id.clone(),
            TerminalSize::new(session.columns, session.rows),
            Box::new(writer),
            information.pty,
        );
        Ok(Self {
            id: session.id,
            working_directory: session.working_directory,
            transport,
            renderer,
            write_failure,
            closed: false,
            owner: attached.owner,
        })
    }
    pub fn snapshot(&self) -> &TerminalScreen {
        self.renderer.snapshot()
    }

    fn ensure_open(&self) -> Result<(), String> {
        if self.closed {
            Err("Terminal は終了しています".into())
        } else {
            Ok(())
        }
    }

    /// Host が返す raw bytes と出力位置を検証してから描画状態を進める。
    pub fn poll(&mut self, timeout_ms: u32) -> Result<NativeOutput, String> {
        self.ensure_open()?;
        let (ticket, input) = self
            .renderer
            .begin(v1::MAX_READ_BYTES, timeout_ms)
            .map_err(|error| error.to_string())?;
        let result = call::<_, v1::ReadResponse>(&self.transport, v1::READ, &input);
        let output = match result {
            Ok(output) => output,
            Err(error) => {
                self.renderer.cancel();
                return Err(error);
            }
        };
        let output = self
            .renderer
            .accept(ticket, output)
            .map_err(|error| error.to_string())?;
        self.owner = output.owner.clone();
        Ok(output)
    }

    /// 入力は renderer の writer が非同期で送る。失敗は take_write_failures で受け取る。
    pub fn input(&mut self, input: &TerminalInput) -> Result<(), String> {
        self.ensure_open()?;
        self.renderer
            .input(input)
            .map_err(|error| error.to_string())
    }

    pub fn resize(
        &mut self,
        size: TerminalSize,
    ) -> Result<(TerminalScreen, TerminalDelta), String> {
        self.ensure_open()?;
        let size = checked_size(size)?;
        call::<_, ()>(
            &self.transport,
            v1::RESIZE,
            &v1::ResizeRequest {
                id: self.id.clone(),
                columns: size.columns,
                rows: size.rows,
            },
        )?;
        Ok(self.renderer.resize(size))
    }

    pub fn take_write_failures(&self) -> Vec<String> {
        let mut failures = self.renderer.take_write_failures().collect::<Vec<_>>();
        if let Ok(mut failure) = self.write_failure.lock()
            && let Some(error) = failure.take()
            && !failures.contains(&error)
        {
            failures.push(error);
        }
        failures
    }

    pub fn claim(&mut self) -> Result<Option<String>, String> {
        self.ensure_open()?;
        let response: v1::OwnerResponse = call(
            &self.transport,
            v1::CLAIM,
            &v1::IdRequest {
                id: self.id.clone(),
            },
        )?;
        self.owner = response.owner.clone();
        Ok(response.owner)
    }

    pub fn release(&mut self) -> Result<Option<String>, String> {
        self.ensure_open()?;
        let response: v1::OwnerResponse = call(
            &self.transport,
            v1::RELEASE,
            &v1::IdRequest {
                id: self.id.clone(),
            },
        )?;
        self.owner = response.owner.clone();
        Ok(response.owner)
    }

    /// 接続を外しても PTY を残す。
    pub fn detach(&mut self) -> Result<(), String> {
        if self.closed {
            return Ok(());
        }
        let _: v1::OwnerResponse = call(
            &self.transport,
            v1::DETACH,
            &v1::IdRequest {
                id: self.id.clone(),
            },
        )?;
        self.closed = true;
        Ok(())
    }

    pub fn close(&mut self) -> Result<(), String> {
        if self.closed {
            return Ok(());
        }
        call::<_, ()>(
            &self.transport,
            v1::CLOSE,
            &v1::IdRequest {
                id: self.id.clone(),
            },
        )?;
        self.closed = true;
        Ok(())
    }
}

fn checked_size(size: TerminalSize) -> Result<TerminalSize, String> {
    let size = terminal_renderer::normalized_size(size);
    if size.columns > 512 || size.rows > 256 {
        return Err("Terminal のサイズが上限を超えています".into());
    }
    Ok(size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, mpsc};

    #[test]
    fn text_and_keys_are_batched_without_losing_unicode_or_reordering() {
        let writes = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
        let observed = writes.clone();
        let transport: NativeTransport = Arc::new(move |request| {
            let result = match request.operation.as_str() {
                v1::INFO => {
                    serde_json::json!({"pty":"posix","default_shell":{"program":"sh","arguments":[]}})
                }
                v1::START => serde_json::json!({"id":"pty-1"}),
                v1::WRITE => {
                    let input: v1::WriteRequest = serde_json::from_str(&request.payload).unwrap();
                    observed.lock().unwrap().push(input.bytes);
                    serde_json::Value::Null
                }
                _ => panic!("unexpected operation {}", request.operation),
            };
            Ok(HostExtensionDispatchResponse {
                id: request.id,
                operation: request.operation,
                result: result.to_string(),
            })
        });
        let mut session =
            NativeSession::start(transport, ".".into(), TerminalSize::new(80, 24), None).unwrap();
        let input = "日本語abc".repeat(200);
        session.input(&TerminalInput::Text(input.clone())).unwrap();
        session
            .input(&TerminalInput::Key(terminal::TerminalKeyEvent {
                key: terminal::TerminalKey::Enter,
                modifiers: Default::default(),
            }))
            .unwrap();
        session.input(&TerminalInput::Text("end".into())).unwrap();
        let expected = format!("{input}\rend").into_bytes();
        let deadline = Instant::now() + Duration::from_secs(10);
        while writes.lock().unwrap().iter().map(Vec::len).sum::<usize>() < expected.len() {
            assert!(
                Instant::now() < deadline,
                "terminal input was not delivered"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let chunks = writes.lock().unwrap();
        assert!(chunks.len() < input.chars().count());
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.len() <= v1::MAX_READ_BYTES as usize)
        );
        assert_eq!(chunks.concat(), expected);
    }

    #[test]
    fn background_write_failure_is_reported_without_another_keypress() {
        let transport: NativeTransport = Arc::new(|request| {
            let result = match request.operation.as_str() {
                v1::INFO => {
                    serde_json::json!({"pty":"posix","default_shell":{"program":"sh","arguments":[]}})
                }
                v1::START => serde_json::json!({"id":"pty-1"}),
                v1::WRITE => return Err("write rejected".into()),
                _ => panic!("unexpected operation {}", request.operation),
            };
            Ok(HostExtensionDispatchResponse {
                id: request.id,
                operation: request.operation,
                result: result.to_string(),
            })
        });
        let mut session =
            NativeSession::start(transport, ".".into(), TerminalSize::new(80, 24), None).unwrap();
        session.input(&TerminalInput::Text("abc".into())).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let errors = session.take_write_failures();
            if !errors.is_empty() {
                assert!(errors.iter().any(|error| error.contains("write rejected")));
                break;
            }
            assert!(Instant::now() < deadline, "write failure was not reported");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn session_uses_host_shell_preserves_position_and_closes_only_explicitly() {
        let operations = Arc::new(Mutex::new(Vec::new()));
        let observed = operations.clone();
        let (writes, receiver) = mpsc::channel();
        let failed_read = Arc::new(Mutex::new(true));
        let fail_once = failed_read.clone();
        let transport: NativeTransport = Arc::new(move |request| {
            observed.lock().unwrap().push(request.operation.clone());
            let result = match request.operation.as_str() {
                v1::INFO => {
                    serde_json::json!({"pty":"conpty","default_shell":{"program":"powershell.exe","arguments":["-NoLogo"]}})
                }
                v1::START => {
                    let input: v1::StartRequest = serde_json::from_str(&request.payload).unwrap();
                    assert_eq!(input.program, "powershell.exe");
                    assert_eq!(input.arguments, ["-NoLogo"]);
                    assert_eq!(input.working_directory, "host-workspace");
                    serde_json::json!({"id":"pty-1"})
                }
                v1::READ => {
                    let input: v1::ReadRequest = serde_json::from_str(&request.payload).unwrap();
                    assert_eq!(input.offset, 0);
                    let mut fail = fail_once.lock().unwrap();
                    if *fail {
                        *fail = false;
                        return Err("temporary disconnect".into());
                    }
                    serde_json::json!({"bytes":[104,105],"next":2,"lost_before":null,"exit":null,"read_error":null,"write_error":null})
                }
                v1::WRITE => {
                    let input: v1::WriteRequest = serde_json::from_str(&request.payload).unwrap();
                    assert_eq!(input.id, "pty-1");
                    writes.send(input.bytes).unwrap();
                    serde_json::Value::Null
                }
                v1::RESIZE | v1::CLOSE => serde_json::Value::Null,
                _ => panic!("unexpected operation"),
            };
            Ok(HostExtensionDispatchResponse {
                id: request.id,
                operation: request.operation,
                result: result.to_string(),
            })
        });
        let mut session = NativeSession::start(
            transport.clone(),
            "host-workspace".into(),
            TerminalSize::new(20, 3),
            None,
        )
        .unwrap();
        assert_eq!(session.id(), "pty-1");
        assert!(session.poll(0).is_err());
        assert!(session.poll(0).unwrap().delta.is_some());
        assert!(session.snapshot().plain_text().contains("hi"));
        session.input(&TerminalInput::Text("x".into())).unwrap();
        assert_eq!(
            receiver
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap(),
            b"x"
        );
        session.resize(TerminalSize::new(30, 4)).unwrap();
        assert_eq!(session.snapshot().size, TerminalSize::new(30, 4));
        session.close().unwrap();
        session.close().unwrap();
        assert!(session.poll(0).is_err());
        assert_eq!(
            operations
                .lock()
                .unwrap()
                .iter()
                .filter(|value| value.as_str() == v1::CLOSE)
                .count(),
            1
        );
        drop(session);
        assert_eq!(
            operations
                .lock()
                .unwrap()
                .iter()
                .filter(|value| value.as_str() == v1::CLOSE)
                .count(),
            1
        );
    }

    #[test]
    fn dropping_client_does_not_close_the_host_session() {
        let operations = Arc::new(Mutex::new(Vec::new()));
        let observed = operations.clone();
        let transport: NativeTransport = Arc::new(move |request| {
            observed.lock().unwrap().push(request.operation.clone());
            let result = match request.operation.as_str() {
                v1::INFO => {
                    serde_json::json!({"pty":"posix","default_shell":{"program":"sh","arguments":[]}})
                }
                v1::START => serde_json::json!({"id":"pty-2"}),
                v1::LIST => {
                    serde_json::json!({"sessions":[{"id":"pty-2","columns":20,"rows":3,"working_directory":"."}]})
                }
                v1::ATTACH => serde_json::json!({"owner":null}),
                v1::READ => {
                    let input: v1::ReadRequest = serde_json::from_str(&request.payload).unwrap();
                    assert_eq!(input.offset, 0);
                    serde_json::json!({"bytes":[104,105],"next":2,"lost_before":null,"exit":null,"read_error":null,"write_error":null})
                }
                _ => panic!("unexpected operation"),
            };
            Ok(HostExtensionDispatchResponse {
                id: request.id,
                operation: request.operation,
                result: result.to_string(),
            })
        });
        let session = NativeSession::start(
            transport.clone(),
            ".".into(),
            TerminalSize::new(20, 3),
            None,
        )
        .unwrap();
        drop(session);
        assert!(
            !operations
                .lock()
                .unwrap()
                .iter()
                .any(|value| value == v1::CLOSE)
        );
        let mut reattached = NativeSession::attach(transport, "pty-2").unwrap();
        assert_eq!(reattached.working_directory(), ".");
        assert!(reattached.poll(0).unwrap().delta.is_some());
        assert!(reattached.snapshot().plain_text().contains("hi"));
    }

    #[test]
    fn selected_shell_profile_keeps_its_environment() {
        let transport: NativeTransport = Arc::new(move |request| {
            let value = match request.operation.as_str() {
                v1::INFO => {
                    serde_json::json!({"pty":"posix","default_shell":{"program":"sh","arguments":[]}})
                }
                v1::START => {
                    let start: v1::StartRequest = serde_json::from_str(&request.payload).unwrap();
                    assert_eq!(start.program, "fish");
                    assert_eq!(start.arguments, ["-l"]);
                    assert_eq!(
                        start.environment.get("TERM").map(String::as_str),
                        Some("xterm-256color")
                    );
                    serde_json::json!({"id":"pty-profile"})
                }
                _ => panic!("unexpected operation"),
            };
            Ok(HostExtensionDispatchResponse {
                id: request.id,
                operation: request.operation,
                result: value.to_string(),
            })
        });
        let profile = v1::ShellCommand {
            program: "fish".into(),
            arguments: vec!["-l".into()],
            environment: [("TERM".into(), "xterm-256color".into())].into(),
        };
        let session = NativeSession::start(
            transport,
            ".".into(),
            TerminalSize::new(20, 3),
            Some(profile),
        )
        .unwrap();
        assert_eq!(session.id(), "pty-profile");
    }
}
