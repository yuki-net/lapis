//! Local Host の Task worker と永続化。

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use editor_core::ExecutionId;
use local_fs::atomic_json_write;
use serde::{Deserialize, Serialize};
use task_runner::{
    CodexExecutionSpec, ExecutionStatus, RunnerUpdate, TaskBackend, TaskControl, TaskError,
    TaskEvent, TaskRecord, run_codex_app_server,
};

#[derive(Clone)]
pub struct LocalTaskBackend {
    root: PathBuf,
    codex_executable: PathBuf,
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
        }
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
        command.spawn().map_err(|error| {
            TaskError::new(format!("Task worker を起動できませんでした: {error}"))
        })?;
        Ok(())
    }

    fn control(&self, execution_id: &ExecutionId, control: &TaskControl) -> Result<(), TaskError> {
        atomic_json_write(&self.control_path(execution_id), control)?;
        Ok(())
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
    let run_result = run_codex_app_server(&spec.runner, control_rx, |update| {
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
    });
    finished.store(true, Ordering::Relaxed);
    let _ = poller.join();
    if let Err(error) = &run_result {
        record.execution.failure = Some(error.to_string());
        record.push_event(TaskEvent::Error {
            message: error.to_string(),
        });
        record.set_status(ExecutionStatus::Failed);
        backend.persist_record(&record)?;
    }
    let _ = fs::remove_file(spec_path);
    let _ = fs::remove_file(backend.control_path(&execution_id));
    run_result
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
