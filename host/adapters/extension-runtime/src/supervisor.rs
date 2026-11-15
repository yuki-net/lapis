//! 拡張ホストの子プロセスを監督する。Host の起動時に確定した bytes を再起動時も使用する。

use std::{
    collections::BTreeMap,
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

use crate::{LIFECYCLE_VERSION, Request, Response};
use base64::{Engine as _, engine::general_purpose::STANDARD};

const RESPONSE_LIMIT: u64 = 64 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(100);

/// 署名と互換性を呼び出し側で確認した component の起動時スナップショット。
#[derive(Clone, Debug)]
pub struct ComponentImage {
    pub id: String,
    pub bytes: Vec<u8>,
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
    cancelled: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    status: Arc<Mutex<SupervisorStatus>>,
}

/// 実行ファイルの存在だけで能力を宣言せず、別プロセスと版を照合する。
pub fn probe_worker(executable: &Path) -> Result<(), String> {
    let mut worker = Worker::spawn(executable)?;
    let result = worker.handshake(None);
    worker.shutdown();
    result
}

impl ExtensionSupervisor {
    /// UI スレッドを待たせないよう、起動・再起動と IPC は監督スレッドで行う。
    pub fn start(executable: PathBuf, components: Vec<ComponentImage>) -> Self {
        let (stop, receiver) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let status = Arc::new(Mutex::new(SupervisorStatus::Starting));
        let state = Arc::clone(&status);
        let thread = thread::spawn(move || {
            supervise(
                &executable,
                &components,
                receiver,
                &worker_cancelled,
                &state,
            )
        });
        Self {
            stop,
            cancelled,
            thread: Some(thread),
            status,
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
    components: &[ComponentImage],
    stop: mpsc::Receiver<()>,
    cancelled: &AtomicBool,
    status: &Mutex<SupervisorStatus>,
) {
    let mut retries = 0;
    loop {
        if cancelled.load(Ordering::Acquire) || stop.try_recv().is_ok() {
            return;
        }
        update(status, SupervisorStatus::Starting);
        let attempt = (|| -> Result<(), String> {
            let mut worker = Worker::spawn(executable)?;
            worker.handshake(Some(cancelled))?;
            let mut active = Vec::new();
            let mut failed = BTreeMap::new();
            for component in components {
                if stop.try_recv().is_ok() {
                    worker.shutdown();
                    return Ok(());
                }
                let response = worker.request(
                    Request::Activate {
                        id: component.id.clone(),
                        component: STANDARD.encode(&component.bytes),
                    },
                    REQUEST_TIMEOUT,
                    Some(cancelled),
                )?;
                if response.ok {
                    active.push(component.id.clone());
                } else {
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
                        worker.shutdown();
                        return Ok(());
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                if let Some(exit) = worker.child.try_wait().map_err(|error| error.to_string())? {
                    return Err(format!("拡張ホストが終了しました: {exit}"));
                }
            }
        })();
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
    responses: mpsc::Receiver<Result<Response, String>>,
}

impl Worker {
    fn handshake(&mut self, cancelled: Option<&AtomicBool>) -> Result<(), String> {
        let response = self.request(Request::Handshake, Duration::from_secs(3), cancelled)?;
        if response.ok && response.lifecycle_version.as_deref() == Some(LIFECYCLE_VERSION) {
            Ok(())
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
                        serde_json::from_slice::<Response>(&line)
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
        })
    }

    fn request(
        &mut self,
        request: Request,
        timeout: Duration,
        cancelled: Option<&AtomicBool>,
    ) -> Result<Response, String> {
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
                Ok(response) => return response,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("拡張ホストが切断されました".to_owned());
                }
            }
        }
    }

    fn shutdown(&mut self) {
        let _ = self.request(Request::Shutdown, Duration::from_millis(500), None);
        self.terminate();
    }

    fn terminate(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.terminate();
    }
}
