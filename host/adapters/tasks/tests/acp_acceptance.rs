//! ACP 経由で実ハーネス（OpenCode）を起動する acceptance 検証。
//!
//! 本番と同じ `run_task_worker` を in-process で実行し、永続化された
//! `TaskRecord` に agent の応答が残ることを確かめる。
//!
//! OpenCode が無い環境（CI を含む）では skip する。決定的な client 側の検証は
//! `local-acp` の `fake-acp-agent` を使った統合テストが担当する。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use editor_core::{ConversationId, ExecutionId, TaskId, WorkspaceId};
use local_acp::opencode_spec;
use local_tasks::{LocalTaskBackend, run_task_worker};
use task::{Execution, ExecutionStatus, Task, TaskBackend, TaskRecord};

/// OpenCode の実行ファイルが見つからなければ `None`。
fn opencode() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("LAPIS_OPENCODE_PATH") {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    let path = opencode_spec();
    path.is_file().then_some(path)
}

#[test]
fn opencode_answers_a_prompt_through_the_acp_boundary() {
    let Some(executable) = opencode() else {
        eprintln!("OpenCode が見つからないため ACP acceptance 検証を skip します");
        return;
    };

    let root = tempfile::tempdir().expect("task storage");
    let workspace = tempfile::tempdir().expect("workspace");
    fs::write(workspace.path().join("README.md"), "# demo\n").expect("fixture");

    let spec_directory = tempfile::tempdir().expect("spec directory");
    let spec_path = spec_directory.path().join("start.json");

    let task_id = TaskId::new("task-acceptance");
    let execution_id = ExecutionId::new("execution-acceptance");
    let record = TaskRecord::new(
        Task {
            id: task_id.clone(),
            conversation_id: ConversationId::new("conversation-acceptance"),
            title: "acceptance".to_owned(),
            prompt: "Reply with exactly the word pong. Do not use tools.".to_owned(),
            created_at_ms: 1,
        },
        Execution {
            id: execution_id.clone(),
            task_id,
            workspace_id: WorkspaceId::new("workspace-acceptance"),
            workspace_root: workspace.path().to_owned(),
            runner: "acp-opencode".to_owned(),
            mode: task::TaskMode::Default,
            status: ExecutionStatus::Queued,
            started_at_ms: None,
            completed_at_ms: None,
            external_thread_id: None,
            failure: None,
        },
    );

    let worker_spec = serde_json::json!({
        "root": root.path(),
        "record": record,
        "runner": {
            "executable": executable,
            "prompt": "Reply with exactly the word pong. Do not use tools.",
            "workspace_root": workspace.path(),
            "mode": "default",
            "runner": "acp-opencode",
        }
    });
    fs::write(&spec_path, serde_json::to_vec(&worker_spec).unwrap()).expect("write spec");

    // 本番の worker と同じ関数を in-process で走らせる。
    let path = spec_path.clone();
    let handle = thread::spawn(move || run_task_worker(&path));
    let storage = Arc::new(root.path().to_owned());
    let record = wait_for_terminal(&storage, &execution_id);
    let result = handle.join().expect("worker thread");
    assert!(result.is_ok(), "worker failed: {result:?}");

    let record = record.expect("execution record");
    assert_eq!(
        record.execution.status,
        ExecutionStatus::Succeeded,
        "execution: {:?}",
        record.execution
    );
    assert!(
        record.execution.external_thread_id.is_some(),
        "ACP session id must be recorded"
    );
    let text = record
        .events
        .iter()
        .map(|event| event.event.display_text())
        .collect::<String>();
    // display までのデータの流れを可視化する（失敗時のみ表示される）。
    println!("{text}");
    assert!(
        text.to_lowercase().contains("pong"),
        "agent output must be displayed: {text:?}"
    );
}

fn wait_for_terminal(storage: &Path, execution_id: &ExecutionId) -> Option<TaskRecord> {
    let backend = LocalTaskBackend::new(storage.to_owned(), PathBuf::from("unused"));
    let deadline = Instant::now() + Duration::from_secs(180);
    while Instant::now() < deadline {
        let record = backend
            .load()
            .ok()?
            .into_iter()
            .find(|record| &record.execution.id == execution_id);
        if let Some(record) = record
            && record.execution.status.is_terminal()
        {
            return Some(record);
        }
        thread::sleep(Duration::from_millis(200));
    }
    None
}
