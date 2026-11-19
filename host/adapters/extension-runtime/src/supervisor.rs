//! 拡張ホストの子プロセスを監督する。Host の起動時に確定した bytes を再起動時も使用する。

use std::{
    collections::{BTreeMap, VecDeque},
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crate::{
    DISPATCH_VERSION, HostCall, HostReply, InvocationContext, LIFECYCLE_VERSION,
    MAX_DISPATCH_BYTES, RELEASE_CLAIMS_OPERATION, Request, Response, SESSION_DETACHED_OPERATION,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};

const RESPONSE_LIMIT: u64 = 512 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(100);
const TERMINATION_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_PENDING_DISCONNECTS: usize = 1024;

#[derive(Default)]
struct Disconnects {
    pending: VecDeque<InvocationContext>,
    release_all: bool,
}

impl Disconnects {
    fn push(&mut self, context: InvocationContext) {
        if self.pending.len() >= MAX_PENDING_DISCONNECTS {
            self.pending.clear();
            self.release_all = true;
        } else if !self.release_all {
            self.pending.push_back(context);
        }
    }

    fn pop(&mut self) -> Option<(&'static str, InvocationContext)> {
        if std::mem::take(&mut self.release_all) {
            Some((
                RELEASE_CLAIMS_OPERATION,
                InvocationContext {
                    session: "host-internal".into(),
                    workspace: "host".into(),
                },
            ))
        } else {
            self.pending
                .pop_front()
                .map(|context| (SESSION_DETACHED_OPERATION, context))
        }
    }
}

#[cfg(test)]
mod disconnect_tests {
    use super::*;

    #[test]
    fn overflow_coalesces_and_bounds_disconnects() {
        let context = InvocationContext {
            session: "session".into(),
            workspace: "workspace".into(),
        };
        let mut events = Disconnects::default();
        events.push(context.clone());
        let (operation, delivered) = events.pop().unwrap();
        assert_eq!(operation, SESSION_DETACHED_OPERATION);
        assert_eq!(delivered.session, context.session);
        for _ in 0..MAX_PENDING_DISCONNECTS + 10 {
            events.push(context.clone());
        }
        assert!(events.pending.is_empty());
        assert_eq!(events.pop().unwrap().0, RELEASE_CLAIMS_OPERATION);
        assert!(events.pop().is_none());
        events.push(context);
        assert_eq!(events.pop().unwrap().0, SESSION_DETACHED_OPERATION);
    }
}

struct Dispatch {
    id: String,
    operation: String,
    payload: String,
    context: InvocationContext,
    abandoned: Arc<AtomicBool>,
    reply: mpsc::Sender<Result<String, String>>,
}

/// Host 側で検証済みの capability を処理する。未対応の操作は拒否する。
pub trait HostCallHandler: Send + Sync {
    fn call(
        &self,
        extension: &str,
        method: &str,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, String>;
    /// dispatch 中の能力呼び出しに Host が認証した接続情報を付与する。
    fn call_in_context(
        &self,
        extension: &str,
        method: &str,
        payload: serde_json::Value,
        _context: Option<&InvocationContext>,
    ) -> Result<serde_json::Value, String> {
        self.call(extension, method, payload)
    }
    fn release(&self, _extension: Option<&str>) {}
}

/// 署名と互換性を呼び出し側で確認した component の起動時スナップショット。
#[derive(Clone, Debug)]
pub struct ComponentImage {
    pub id: String,
    pub bytes: Vec<u8>,
    pub require_dispatch: bool,
}

#[derive(Clone)]
struct RuntimeImage {
    image: ComponentImage,
    native: bool,
}

/// 現在動いているものと、次回起動時の設定を混同しないための実行状態。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SupervisorStatus {
    Starting,
    Running {
        pid: u32,
        active: Vec<String>,
        failed: BTreeMap<String, String>,
    },
    Stopped {
        reason: String,
    },
}

pub struct ExtensionSupervisor {
    stop: mpsc::Sender<()>,
    calls: mpsc::SyncSender<Dispatch>,
    cancelled: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    status: Arc<Mutex<SupervisorStatus>>,
    disconnects: Arc<Mutex<Disconnects>>,
}

/// 実行ファイルの存在だけで能力を宣言せず、別プロセスと版を照合する。
pub fn probe_worker(executable: &Path) -> Result<Response, String> {
    let mut worker = Worker::spawn(executable)?;
    let result = worker.handshake(None);
    worker.shutdown(None);
    result
}

impl ExtensionSupervisor {
    /// UI スレッドを待たせないよう、起動・再起動と IPC は監督スレッドで行う。
    pub fn start(executable: PathBuf, components: Vec<ComponentImage>) -> Self {
        Self::start_with_handler(executable, components, None)
    }

    pub fn start_with_handler(
        executable: PathBuf,
        components: Vec<ComponentImage>,
        handler: Option<Arc<dyn HostCallHandler>>,
    ) -> Self {
        Self::start_images(
            executable,
            components
                .into_iter()
                .map(|image| RuntimeImage {
                    image,
                    native: false,
                })
                .collect(),
            handler,
        )
    }

    /// 署名・互換性検証済み native image を別プロセスに渡す。
    ///
    /// # Safety
    /// 全 native image が信頼できる実行コードであることを呼び出し側が保証する。
    #[cfg(windows)]
    pub unsafe fn start_with_verified_native(
        executable: PathBuf,
        components: Vec<ComponentImage>,
        native: Vec<ComponentImage>,
        handler: Option<Arc<dyn HostCallHandler>>,
    ) -> Self {
        let images = components
            .into_iter()
            .map(|image| RuntimeImage {
                image,
                native: false,
            })
            .chain(native.into_iter().map(|mut image| {
                image.require_dispatch = true;
                RuntimeImage {
                    image,
                    native: true,
                }
            }))
            .collect();
        Self::start_images(executable, images, handler)
    }

    fn start_images(
        executable: PathBuf,
        components: Vec<RuntimeImage>,
        handler: Option<Arc<dyn HostCallHandler>>,
    ) -> Self {
        let (stop, receiver) = mpsc::channel();
        let (calls, call_receiver) = mpsc::sync_channel(16);
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let status = Arc::new(Mutex::new(SupervisorStatus::Starting));
        let disconnects = Arc::new(Mutex::new(Disconnects::default()));
        let pending_disconnects = Arc::clone(&disconnects);
        let state = Arc::clone(&status);
        let thread = thread::spawn(move || {
            supervise(
                &executable,
                &components,
                receiver,
                call_receiver,
                (&worker_cancelled, &pending_disconnects),
                &state,
                handler.as_deref(),
            )
        });
        Self {
            stop,
            calls,
            cancelled,
            thread: Some(thread),
            status,
            disconnects,
        }
    }

    /// 切断通知は I/O を行わず監督スレッドへ渡す。過負荷時は所有権のみ解放する。
    pub fn session_disconnected(&self, context: InvocationContext) {
        if !context.valid() {
            return;
        }
        if let Ok(mut events) = self.disconnects.lock() {
            events.push(context);
        }
    }

    /// 接続に属さない Host 内部処理専用。外部の要求は dispatch_with_context を使う。
    /// 同期呼び出しのため UI スレッドから直接実行しない。
    pub fn dispatch_host_internal(
        &self,
        id: &str,
        operation: &str,
        payload: &str,
    ) -> Result<String, String> {
        self.dispatch_with_context(
            id,
            operation,
            payload,
            &InvocationContext {
                session: "host-internal".into(),
                workspace: "host".into(),
            },
        )
    }

    pub fn dispatch_with_context(
        &self,
        id: &str,
        operation: &str,
        payload: &str,
        context: &InvocationContext,
    ) -> Result<String, String> {
        if !context.valid()
            || id.len() > 128
            || operation.is_empty()
            || operation.len() > 256
            || payload.len() > MAX_DISPATCH_BYTES
        {
            return Err("invalid dispatch request".into());
        }
        let (reply, receiver) = mpsc::channel();
        let abandoned = Arc::new(AtomicBool::new(false));
        self.calls
            .try_send(Dispatch {
                id: id.into(),
                operation: operation.into(),
                payload: payload.into(),
                context: context.clone(),
                abandoned: Arc::clone(&abandoned),
                reply,
            })
            .map_err(|_| "extension dispatch queue unavailable".to_owned())?;
        match receiver.recv_timeout(REQUEST_TIMEOUT + Duration::from_secs(1)) {
            Ok(result) => result,
            Err(_) => {
                abandoned.store(true, Ordering::Release);
                Err("extension dispatch timed out; operation may have executed".into())
            }
        }
    }

    pub fn status(&self) -> SupervisorStatus {
        self.status.lock().map_or_else(
            |_| SupervisorStatus::Stopped {
                reason: "拡張ホストの状態を取得できません".to_owned(),
            },
            |status| status.clone(),
        )
    }
}

impl Drop for ExtensionSupervisor {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        let _ = self.stop.send(());
        // 監督スレッドの再起動・応答は時間制限付き。終了処理中も必ず子を回収する。
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn update(status: &Mutex<SupervisorStatus>, value: SupervisorStatus) {
    if let Ok(mut status) = status.lock() {
        *status = value;
    }
}

fn supervise(
    executable: &Path,
    components: &[RuntimeImage],
    stop: mpsc::Receiver<()>,
    calls: mpsc::Receiver<Dispatch>,
    control: (&AtomicBool, &Mutex<Disconnects>),
    status: &Mutex<SupervisorStatus>,
    handler: Option<&dyn HostCallHandler>,
) {
    let (cancelled, disconnects) = control;
    let mut retries = 0;
    loop {
        if cancelled.load(Ordering::Acquire) || stop.try_recv().is_ok() {
            return;
        }
        update(status, SupervisorStatus::Starting);
        let attempt = (|| -> Result<(), String> {
            let mut worker = Worker::spawn(executable)?;
            let handshake = worker.handshake(Some(cancelled))?;
            let mut active = Vec::new();
            let mut failed = BTreeMap::new();
            for entry in components {
                let component = &entry.image;
                if stop.try_recv().is_ok() {
                    worker.shutdown(handler);
                    return Ok(());
                }
                if entry.native && handshake.native_version.as_deref() != Some("1.0.0") {
                    failed.insert(component.id.clone(), "native ABI v1 is unsupported".into());
                    continue;
                }
                let request = if entry.native {
                    Request::ActivateNative {
                        id: component.id.clone(),
                        image: STANDARD.encode(&component.bytes),
                    }
                } else {
                    Request::Activate {
                        id: component.id.clone(),
                        component: STANDARD.encode(&component.bytes),
                        require_dispatch: component.require_dispatch,
                    }
                };
                let response = worker.request(
                    request,
                    REQUEST_TIMEOUT,
                    Some(cancelled),
                    handler,
                    Some(&component.id),
                )?;
                if response.ok {
                    active.push(component.id.clone());
                    worker.active.push(component.id.clone());
                } else {
                    if let Some(handler) = handler {
                        handler.release(Some(&component.id));
                    }
                    failed.insert(
                        component.id.clone(),
                        response
                            .error
                            .unwrap_or_else(|| "拡張を起動できません".into()),
                    );
                }
            }
            update(
                status,
                SupervisorStatus::Running {
                    pid: worker.child.id(),
                    active,
                    failed,
                },
            );
            loop {
                match stop.recv_timeout(POLL) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                        worker.shutdown(handler);
                        return Ok(());
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                let notification = disconnects.lock().ok().and_then(|mut events| events.pop());
                if let Some((operation, context)) = notification {
                    let targets: Vec<_> = components
                        .iter()
                        .filter(|entry| {
                            entry.image.require_dispatch && worker.active.contains(&entry.image.id)
                        })
                        .map(|entry| entry.image.id.clone())
                        .collect();
                    for id in targets {
                        let _ = worker.request(
                            Request::Dispatch {
                                id: id.clone(),
                                operation: operation.into(),
                                payload: "{}".into(),
                                context: context.clone(),
                            },
                            REQUEST_TIMEOUT,
                            Some(cancelled),
                            handler,
                            Some(&id),
                        )?;
                    }
                }
                if let Ok(call) = calls.try_recv() {
                    if call.abandoned.load(Ordering::Acquire) {
                        continue;
                    }
                    if !worker.supports_dispatch {
                        let _ = call
                            .reply
                            .send(Err("extension host does not support dispatch v1".into()));
                        continue;
                    }
                    if !worker.active.contains(&call.id) {
                        let _ = call.reply.send(Err("extension is not active".into()));
                        continue;
                    }
                    let result = worker.request(
                        Request::Dispatch {
                            id: call.id.clone(),
                            operation: call.operation,
                            payload: call.payload,
                            context: call.context,
                        },
                        REQUEST_TIMEOUT,
                        Some(cancelled),
                        handler,
                        Some(&call.id),
                    );
                    match result {
                        Ok(response) => {
                            let outcome = if response.ok {
                                response
                                    .result
                                    .ok_or("dispatch response missing result".to_owned())
                            } else {
                                Err(response
                                    .error
                                    .unwrap_or_else(|| "dispatch failed".to_owned()))
                            };
                            let _ = call.reply.send(outcome);
                        }
                        Err(error) => {
                            let _ = call.reply.send(Err(error.clone()));
                            return Err(error);
                        }
                    }
                }
                if let Some(exit) = worker.child.try_wait().map_err(|error| error.to_string())? {
                    return Err(format!("拡張ホストが終了しました: {exit}"));
                }
            }
        })();
        if let Some(handler) = handler {
            handler.release(None);
        }
        if cancelled.load(Ordering::Acquire) {
            return;
        }
        match attempt {
            Ok(()) => return,
            Err(reason) if retries == 0 => {
                retries += 1;
                eprintln!("{reason}。拡張ホストを再起動します");
            }
            Err(reason) => {
                update(status, SupervisorStatus::Stopped { reason });
                return;
            }
        }
    }
}

struct Worker {
    child: Child,
    requests: mpsc::Sender<Vec<u8>>,
    responses: mpsc::Receiver<Result<WorkerEvent, String>>,
    active: Vec<String>,
    supports_dispatch: bool,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum WorkerEvent {
    Call(HostCall),
    Response(Response),
}

impl Worker {
    fn handshake(&mut self, cancelled: Option<&AtomicBool>) -> Result<Response, String> {
        let response = self.request(
            Request::Handshake,
            Duration::from_secs(3),
            cancelled,
            None,
            None,
        )?;
        if response.ok && response.lifecycle_version.as_deref() == Some(LIFECYCLE_VERSION) {
            self.supports_dispatch = response.dispatch_version.as_deref() == Some(DISPATCH_VERSION);
            Ok(response)
        } else {
            Err("拡張ホストのライフサイクル契約が一致しません".to_owned())
        }
    }
    fn spawn(executable: &Path) -> Result<Self, String> {
        let mut command = Command::new(executable);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        let mut child = command
            .spawn()
            .map_err(|error| format!("拡張ホストを起動できません: {error}"))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or("拡張ホストの入力を取得できません")?;
        let stdout = child
            .stdout
            .take()
            .ok_or("拡張ホストの出力を取得できません")?;
        let (requests, input) = mpsc::channel::<Vec<u8>>();
        thread::spawn(move || {
            for bytes in input {
                if stdin.write_all(&bytes).and_then(|_| stdin.flush()).is_err() {
                    break;
                }
            }
        });
        let (output, responses) = mpsc::channel();
        thread::spawn(move || {
            let mut stdout = BufReader::new(stdout);
            loop {
                let mut line = Vec::new();
                let result = match (&mut stdout)
                    .take(RESPONSE_LIMIT + 1)
                    .read_until(b'\n', &mut line)
                {
                    Ok(0) => break,
                    Ok(size) if size as u64 <= RESPONSE_LIMIT => {
                        serde_json::from_slice::<WorkerEvent>(&line)
                            .map_err(|_| "拡張ホストの応答が不正です".to_owned())
                    }
                    Ok(_) => Err("拡張ホストの応答が大きすぎます".to_owned()),
                    Err(error) => Err(error.to_string()),
                };
                let failed = result.is_err();
                if output.send(result).is_err() || failed {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            requests,
            responses,
            active: Vec::new(),
            supports_dispatch: false,
        })
    }

    fn request(
        &mut self,
        request: Request,
        timeout: Duration,
        cancelled: Option<&AtomicBool>,
        handler: Option<&dyn HostCallHandler>,
        invoking: Option<&str>,
    ) -> Result<Response, String> {
        let context = match &request {
            Request::Dispatch { context, .. } => Some(context.clone()),
            _ => None,
        };
        let mut bytes = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
        bytes.push(b'\n');
        self.requests
            .send(bytes)
            .map_err(|_| "拡張ホストへ送信できません".to_owned())?;
        let deadline = Instant::now() + timeout;
        loop {
            if cancelled.is_some_and(|cancelled| cancelled.load(Ordering::Acquire)) {
                return Err("拡張ホストの停止が要求されました".to_owned());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("拡張ホストが応答しません".to_owned());
            }
            match self.responses.recv_timeout(remaining.min(POLL)) {
                Ok(Ok(WorkerEvent::Response(response))) => return Ok(response),
                Ok(Ok(WorkerEvent::Call(call))) => {
                    let result = if call.kind != "call" || invoking != Some(call.extension.as_str())
                    {
                        Err("能力呼び出しの所有者が一致しません".into())
                    } else if let Some(handler) = handler {
                        handler.call_in_context(
                            &call.extension,
                            &call.method,
                            call.payload,
                            context.as_ref(),
                        )
                    } else {
                        Err("Host capability unavailable".into())
                    };
                    let reply = HostReply {
                        kind: "reply".into(),
                        id: call.id,
                        result,
                    };
                    let mut bytes =
                        serde_json::to_vec(&reply).map_err(|error| error.to_string())?;
                    bytes.push(b'\n');
                    self.requests
                        .send(bytes)
                        .map_err(|_| "能力応答を送信できません".to_owned())?;
                }
                Ok(Err(error)) => return Err(error),
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("拡張ホストが切断されました".to_owned());
                }
            }
        }
    }

    fn shutdown(&mut self, handler: Option<&dyn HostCallHandler>) {
        for id in std::mem::take(&mut self.active) {
            let _ = self.request(
                Request::Deactivate { id: id.clone() },
                Duration::from_millis(500),
                None,
                handler,
                Some(&id),
            );
        }
        let _ = self.request(
            Request::Shutdown,
            Duration::from_millis(500),
            None,
            None,
            None,
        );
        self.terminate();
    }

    fn terminate(&mut self) {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            use std::process::{Command, Stdio};
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            let _ = Command::new("taskkill")
                .args(["/F", "/T", "/PID", &self.child.id().to_string()])
                .creation_flags(CREATE_NO_WINDOW)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.child.kill();
        let deadline = Instant::now() + TERMINATION_TIMEOUT;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) | Err(_) => break,
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(10));
                }
                Ok(None) => break,
            }
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.terminate();
    }
}
