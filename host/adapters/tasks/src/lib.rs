//! Local Host の Task worker と永続化。

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use acp_contract::ACP_RUNNER;
use editor_core::ExecutionId;
use local_fs::atomic_json_write;
use notify::Watcher;
use serde::{Deserialize, Serialize};
use task_runner::{
    CodexExecutionSpec, ExecutionStatus, RunnerUpdate, TaskBackend, TaskControl, TaskError,
    TaskEvent, TaskRecord, run_codex_app_server,
};

use local_acp::{AcpAgentSpec, AcpUpdate, opencode_spec, run_acp_session};

#[derive(Clone)]
pub struct LocalTaskBackend {
    root: PathBuf,
    codex_executable: PathBuf,
    watch: Arc<Mutex<Option<TaskWatch>>>,
    /// Task worker を Resource Manager の所有ツリーとして登録する。
    processes: Option<Arc<dyn process::ProcessRegistry>>,
    workspace_id: Option<editor_core::WorkspaceId>,
}

struct TaskWatch {
    _watcher: notify::RecommendedWatcher,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct TaskWorkerSpec {
    root: PathBuf,
    record: TaskRecord,
    runner: CodexExecutionSpec,
}

impl LocalTaskBackend {
    pub fn new(root: PathBuf, codex_executable: PathBuf) -> Self {
        Self {
            root,
            codex_executable,
            watch: Arc::new(Mutex::new(None)),
            processes: None,
            workspace_id: None,
        }
    }

    /// Task worker の起動を Resource Manager へ登録する。
    pub fn with_processes(
        mut self,
        processes: Arc<dyn process::ProcessRegistry>,
        workspace_id: editor_core::WorkspaceId,
    ) -> Self {
        self.processes = Some(processes);
        self.workspace_id = Some(workspace_id);
        self
    }

    pub fn user_default() -> Self {
        let base = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir);
        let executable = env::var_os("LAPIS_CODEX_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(default_codex_executable);
        Self::new(base.join("Lapis").join("tasks-v1"), executable)
    }

    fn record_path(&self, execution_id: &ExecutionId) -> PathBuf {
        self.root
            .join(format!("execution-{}.json", execution_id.as_str()))
    }

    fn control_path(&self, execution_id: &ExecutionId) -> PathBuf {
        self.root
            .join(format!("control-{}.json", execution_id.as_str()))
    }

    fn spec_path(&self, execution_id: &ExecutionId) -> PathBuf {
        self.root
            .join(format!("start-{}.json", execution_id.as_str()))
    }

    fn persist_record(&self, record: &TaskRecord) -> Result<(), TaskError> {
        atomic_json_write(&self.record_path(&record.execution.id), record)?;
        Ok(())
    }

    fn take_control(&self, execution_id: &ExecutionId) -> Result<Option<TaskControl>, TaskError> {
        let path = self.control_path(execution_id);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(TaskError::new(error.to_string())),
        };
        let control = serde_json::from_slice(&bytes)?;
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(TaskError::new(error.to_string())),
        }
        Ok(Some(control))
    }
}

impl TaskBackend for LocalTaskBackend {
    fn load(&self) -> Result<Vec<TaskRecord>, TaskError> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(TaskError::new(error.to_string())),
        };
        let mut records = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| TaskError::new(error.to_string()))?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with("execution-") || !name.ends_with(".json") {
                continue;
            }
            let bytes = fs::read(entry.path())?;
            records.push(serde_json::from_slice(&bytes)?);
        }
        records.sort_by_key(|record: &TaskRecord| record.task.created_at_ms);
        records.reverse();
        Ok(records)
    }

    fn start(&self, record: &TaskRecord) -> Result<(), TaskError> {
        fs::create_dir_all(&self.root)?;
        self.persist_record(record)?;
        let worker_spec = TaskWorkerSpec {
            root: self.root.clone(),
            record: record.clone(),
            runner: CodexExecutionSpec {
                executable: self.codex_executable.clone(),
                prompt: record.task.prompt.clone(),
                workspace_root: record.execution.workspace_root.clone(),
                mode: record.execution.mode,
                runner: Some(record.execution.runner.clone()),
            },
        };
        let spec_path = self.spec_path(&record.execution.id);
        atomic_json_write(&spec_path, &worker_spec)?;
        let current_executable = env::current_exe()?;
        let mut command = Command::new(current_executable);
        command
            .arg("--task-worker")
            .arg(&spec_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_detached_process(&mut command);
        let child = command.spawn().map_err(|error| {
            TaskError::new(format!("Task worker を起動できませんでした: {error}"))
        })?;
        if let (Some(processes), Some(workspace)) = (&self.processes, &self.workspace_id) {
            let _ = processes.register(process::ProcessRootSpec {
                workspace: Some(workspace.clone()),
                kind: process::ProcessKind::TaskRun,
                owner: process::ProcessOwner::task_run(record.execution.id.as_str()),
                pid: child.id(),
                label: record.task.title.clone(),
            });
        }
        Ok(())
    }

    fn control(&self, execution_id: &ExecutionId, control: &TaskControl) -> Result<(), TaskError> {
        atomic_json_write(&self.control_path(execution_id), control)?;
        Ok(())
    }

    fn changes(&self) -> Option<mpsc::Receiver<()>> {
        let mut slot = self.watch.lock().ok()?;
        if slot.is_some() {
            return None;
        }
        fs::create_dir_all(&self.root).ok()?;
        // worker の保存を即座に Client へ伝える。通知は1件で足りる。
        let (sender, receiver) = mpsc::sync_channel(1);
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let changed = match event {
                    Ok(event) => !matches!(event.kind, notify::EventKind::Access(_)),
                    // 取りこぼしの可能性があるため、再同期を促す。
                    Err(_) => true,
                };
                if changed {
                    let _ = sender.try_send(());
                }
            })
            .ok()?;
        watcher
            .watch(&self.root, notify::RecursiveMode::NonRecursive)
            .ok()?;
        *slot = Some(TaskWatch { _watcher: watcher });
        Some(receiver)
    }
}

pub fn run_task_worker(spec_path: &Path) -> Result<(), TaskError> {
    let bytes = fs::read(spec_path)?;
    let spec: TaskWorkerSpec = serde_json::from_slice(&bytes)?;
    let backend = LocalTaskBackend::new(spec.root, spec.runner.executable.clone());
    let mut record = spec.record;
    let execution_id = record.execution.id.clone();
    let (control_tx, control_rx) = mpsc::channel();
    let finished = Arc::new(AtomicBool::new(false));
    let poll_finished = finished.clone();
    let poll_backend = backend.clone();
    let poll_execution = execution_id.clone();
    let poller = thread::spawn(move || {
        while !poll_finished.load(Ordering::Relaxed) {
            match poll_backend.take_control(&poll_execution) {
                Ok(Some(control)) => {
                    if control_tx.send(control).is_err() {
                        break;
                    }
                }
                Ok(None) => {}
                Err(_) => {}
            }
            thread::sleep(Duration::from_millis(100));
        }
    });

    let mut last_persist = Instant::now();
    let runner_id = spec.runner.runner.as_deref();
    let run_result = if is_acp_runner_id(runner_id) {
        let acp_spec = AcpAgentSpec {
            executable: opencode_spec(),
            prompt: spec.runner.prompt.clone(),
            workspace_root: spec.runner.workspace_root.clone(),
        };
        let mut persist_error = None;
        let result = run_acp_session(&acp_spec, control_rx, |update| {
            let urgent = apply_acp_update(&mut record, update);
            if urgent || last_persist.elapsed() >= Duration::from_millis(75) {
                // 保存失敗は最後にまとめて返す。途中では実行を止めない。
                if let Err(error) = backend.persist_record(&record) {
                    persist_error.get_or_insert(error);
                }
                last_persist = Instant::now();
            }
        })
        .map_err(|error| TaskError::new(error.to_string()));
        match (result, persist_error) {
            (Err(error), _) => Err(error),
            (Ok(()), Some(error)) => Err(error),
            (Ok(()), None) => Ok(()),
        }
    } else if is_codex_runner_id(runner_id) {
        run_codex_app_server(&spec.runner, control_rx, |update| {
            let urgent = match update {
                RunnerUpdate::Status(status) => {
                    record.set_status(status);
                    true
                }
                RunnerUpdate::Event(event) => {
                    let urgent = matches!(
                        event,
                        TaskEvent::InputRequested { .. }
                            | TaskEvent::ApprovalRequested { .. }
                            | TaskEvent::Error { .. }
                    );
                    record.push_event(event);
                    urgent
                }
                RunnerUpdate::ExternalThread(id) => {
                    record.execution.external_thread_id = Some(id);
                    true
                }
                RunnerUpdate::Failure(message) => {
                    record.execution.failure = Some(message);
                    true
                }
            };
            if urgent || last_persist.elapsed() >= Duration::from_millis(75) {
                let _ = backend.persist_record(&record);
                last_persist = Instant::now();
            }
        })
    } else {
        // 未知の runner を別の実行系として起動しない。記録と実行を一致させる。
        Err(TaskError::new(format!(
            "未対応の runner です: {}",
            spec.runner.runner.as_deref().unwrap_or("(default)")
        )))
    };
    finished.store(true, Ordering::Relaxed);
    let _ = poller.join();
    if let Err(error) = &run_result {
        record.execution.failure = Some(error.to_string());
        record.push_event(TaskEvent::Error {
            message: error.to_string(),
        });
        record.set_status(ExecutionStatus::Failed);
        // 失敗時の保存まで失敗したら、開始仕様を残して再実行の手掛かりにする。
        backend.persist_record(&record)?;
    } else {
        // 正常終了でも、終端状態の保存成功を確認してから完了とする。
        backend.persist_record(&record)?;
    }
    let _ = fs::remove_file(spec_path);
    let _ = fs::remove_file(backend.control_path(&execution_id));
    run_result
}

/// 実行系の識別子から分岐する。ACP でも Codex でもない識別子は起動しない。
fn is_acp_runner_id(runner: Option<&str>) -> bool {
    runner == Some(ACP_RUNNER)
}

fn is_codex_runner_id(runner: Option<&str>) -> bool {
    matches!(runner, None | Some("codex-app-server"))
}

fn apply_acp_update(record: &mut TaskRecord, update: AcpUpdate) -> bool {
    match update {
        AcpUpdate::Status(status) => {
            record.set_status(status);
            true
        }
        AcpUpdate::Event(event) => {
            let urgent = matches!(
                event,
                TaskEvent::ApprovalRequested { .. } | TaskEvent::Error { .. }
            );
            record.push_event(event);
            urgent
        }
        AcpUpdate::ExternalThread(id) => {
            record.execution.external_thread_id = Some(id);
            true
        }
        AcpUpdate::Failure(message) => {
            record.execution.failure = Some(message);
            true
        }
    }
}

fn default_codex_executable() -> PathBuf {
    if cfg!(windows)
        && let Some(app_data) = env::var_os("APPDATA")
    {
        let candidate = PathBuf::from(app_data).join("npm").join("codex.cmd");
        if candidate.is_file() {
            return candidate;
        }
    }
    PathBuf::from(if cfg!(windows) { "codex.cmd" } else { "codex" })
}

#[cfg(windows)]
fn configure_detached_process(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn configure_detached_process(_command: &mut Command) {}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_core::{ConversationId, TaskId};
    use task::{Execution, Task};

    fn record() -> TaskRecord {
        let task_id = TaskId::new("task-acp");
        TaskRecord::new(
            Task {
                id: task_id.clone(),
                conversation_id: ConversationId::new("conversation-acp"),
                title: "agent".to_owned(),
                prompt: "hi".to_owned(),
                created_at_ms: 1,
            },
            Execution {
                id: ExecutionId::new("execution-acp"),
                task_id,
                workspace_id: editor_core::WorkspaceId::new("workspace-acp"),
                workspace_root: PathBuf::from("workspace"),
                runner: ACP_RUNNER.to_owned(),
                mode: task::TaskMode::Default,
                status: ExecutionStatus::Queued,
                started_at_ms: None,
                completed_at_ms: None,
                external_thread_id: None,
                failure: None,
            },
        )
    }

    #[test]
    fn acp_status_and_thread_updates_reach_the_record() {
        let mut record = record();
        assert!(apply_acp_update(
            &mut record,
            AcpUpdate::ExternalThread("ses-1".to_owned())
        ));
        assert!(apply_acp_update(
            &mut record,
            AcpUpdate::Status(ExecutionStatus::Running)
        ));
        assert_eq!(
            record.execution.external_thread_id.as_deref(),
            Some("ses-1")
        );
        assert_eq!(record.execution.status, ExecutionStatus::Running);
        assert!(record.execution.started_at_ms.is_some());
    }

    #[test]
    fn acp_message_chunks_become_displayable_events() {
        let mut record = record();
        apply_acp_update(
            &mut record,
            AcpUpdate::Event(TaskEvent::MessageDelta {
                text: "pong".to_owned(),
            }),
        );
        assert_eq!(record.events.len(), 1);
        assert_eq!(record.events[0].event.display_text(), "pong");

        // 連続する delta は1件のイベントへ合成される。
        apply_acp_update(
            &mut record,
            AcpUpdate::Event(TaskEvent::MessageDelta {
                text: "!".to_owned(),
            }),
        );
        assert_eq!(record.events.len(), 1);
        assert_eq!(record.events[0].event.display_text(), "pong!");
    }

    #[test]
    fn acp_approval_requests_are_persisted_immediately() {
        let mut record = record();
        let urgent = apply_acp_update(
            &mut record,
            AcpUpdate::Event(TaskEvent::ApprovalRequested {
                request_id: "call-1".to_owned(),
                summary: "write file".to_owned(),
            }),
        );
        assert!(urgent);
        assert!(matches!(
            record.events[0].event,
            TaskEvent::ApprovalRequested { .. }
        ));
    }

    #[test]
    fn acp_failures_are_recorded_on_the_execution() {
        let mut record = record();
        assert!(apply_acp_update(
            &mut record,
            AcpUpdate::Failure("boom".to_owned())
        ));
        assert_eq!(record.execution.failure.as_deref(), Some("boom"));
    }

    #[test]
    fn only_the_acp_runner_selects_the_acp_path() {
        assert_eq!(record().execution.runner, ACP_RUNNER);
        assert_ne!(record().execution.runner, "codex-app-server");
    }

    #[test]
    fn unknown_runners_do_not_fall_back_to_codex() {
        assert!(!is_acp_runner_id(Some("acp-opencod")));
        assert!(!is_codex_runner_id(Some("acp-opencod")));

        assert!(is_codex_runner_id(Some("codex-app-server")));
        assert!(is_codex_runner_id(None));
    }

    #[test]
    fn a_typo_of_the_acp_runner_is_not_treated_as_acp() {
        assert!(is_acp_runner_id(Some(ACP_RUNNER)));
        assert!(!is_codex_runner_id(Some(ACP_RUNNER)));
    }

    /// 未知の runner は Codex でも ACP でもないため、開始仕様から分岐できない。
    #[test]
    fn an_unknown_runner_selects_no_execution_path() {
        let spec = worker_spec("acp-opencod");
        let runner_id = spec.runner.runner.as_deref();
        assert!(!is_acp_runner_id(runner_id));
        assert!(!is_codex_runner_id(runner_id));
    }

    fn worker_spec(runner: &str) -> TaskWorkerSpec {
        TaskWorkerSpec {
            root: PathBuf::from("tasks"),
            record: record(),
            runner: CodexExecutionSpec {
                executable: PathBuf::from("agent"),
                prompt: "hi".to_owned(),
                workspace_root: PathBuf::from("workspace"),
                mode: task::TaskMode::Default,
                runner: Some(runner.to_owned()),
            },
        }
    }
}
