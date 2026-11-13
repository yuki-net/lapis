//! ACP client 境界の end-to-end 検証。
//!
//! `tests/fake-acp-agent` を実際に stdio で起動し、prompt の送信・更新通知の
//! 受信・実行状態の遷移が `task` の値として届くことを確かめる。
//! 実ハーネス（OpenCode）の有無に依存しない。

use std::path::PathBuf;
use std::sync::mpsc;

use local_acp::{AcpAgentSpec, AcpUpdate, run_acp_session};
use task::{ExecutionStatus, TaskEvent};

/// テスト用 agent の実行ファイル。cargo が同じ target ディレクトリへ置く。
fn fake_agent() -> PathBuf {
    // 統合テストの実行ファイルは `target/<profile>/deps/` に置かれる。
    let exe = std::env::current_exe().expect("test executable path");
    let dir = exe
        .parent()
        .and_then(|dir| dir.parent())
        .expect("target dir");
    let name = if cfg!(windows) {
        "fake-acp-agent.exe"
    } else {
        "fake-acp-agent"
    };
    dir.join(name)
}

#[test]
fn acp_client_sends_a_prompt_and_receives_updates() {
    let executable = fake_agent();
    assert!(
        executable.is_file(),
        "fake ACP agent is missing: {}",
        executable.display()
    );

    let workspace = tempfile::tempdir().expect("workspace");
    let spec = AcpAgentSpec {
        executable,
        prompt: "say pong".to_owned(),
        workspace_root: workspace.path().to_owned(),
    };
    let (_control_tx, control_rx) = mpsc::channel();

    let mut updates = Vec::new();
    run_acp_session(&spec, control_rx, |update| updates.push(update)).expect("ACP session");

    assert!(
        updates.iter().any(|update| matches!(
            update,
            AcpUpdate::ExternalThread(id) if id == "ses_fake"
        )),
        "agent session id must be recorded: {updates:?}"
    );
    assert!(
        updates
            .iter()
            .any(|update| matches!(update, AcpUpdate::Status(ExecutionStatus::Running))),
        "the execution must pass through running: {updates:?}"
    );
    assert!(
        updates.iter().any(|update| matches!(
            update,
            AcpUpdate::Event(TaskEvent::MessageDelta { text }) if text == "pong"
        )),
        "the agent message must be displayed: {updates:?}"
    );
    assert!(
        updates.iter().any(|update| matches!(
            update,
            AcpUpdate::Event(TaskEvent::MessageDelta { text }) if text.contains("read README")
        )),
        "tool calls must be displayed: {updates:?}"
    );
    assert!(
        updates
            .iter()
            .any(|update| matches!(update, AcpUpdate::Status(ExecutionStatus::Succeeded))),
        "a completed prompt must succeed: {updates:?}"
    );
}

/// 承認要求 A・B が届き、agent が追加の通知を送らなくても両方に応答できること。
///
/// 応答のたびに読み取りループが起床して次の要求を表示する必要がある。
#[test]
fn both_pending_approvals_can_be_answered_without_further_agent_traffic() {
    let executable = fake_agent();
    assert!(
        executable.is_file(),
        "fake ACP agent is missing: {}",
        executable.display()
    );

    let workspace = tempfile::tempdir().expect("workspace");
    let spec = AcpAgentSpec {
        executable,
        prompt: "send approvals".to_owned(),
        workspace_root: workspace.path().to_owned(),
    };
    let (control_tx, control_rx) = mpsc::channel();

    let updates = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = updates.clone();
    let worker = std::thread::spawn(move || {
        run_acp_session(&spec, control_rx, |update| {
            sink.lock().unwrap().push(update);
        })
    });

    // 1 件目の承認待ちが出るまで待つ。
    wait_for_approvals(&updates, 1);
    control_tx.send(task::TaskControl::Approve).unwrap();
    // 2 件目が再表示されるまで待つ（agent からの追加通知は無い）。
    wait_for_approvals(&updates, 2);
    control_tx.send(task::TaskControl::Approve).unwrap();

    let result = worker.join().expect("worker thread");
    assert!(result.is_ok(), "ACP session failed: {result:?}");

    let updates = updates.lock().unwrap();
    let approvals = updates
        .iter()
        .filter(|update| {
            matches!(
                update,
                AcpUpdate::Event(TaskEvent::ApprovalRequested { .. })
            )
        })
        .count();
    assert!(
        approvals >= 2,
        "both approvals must be displayed: {updates:?}"
    );
    assert!(
        updates
            .iter()
            .any(|update| matches!(update, AcpUpdate::Status(ExecutionStatus::Succeeded))),
        "the prompt must complete after both approvals: {updates:?}"
    );
}

fn wait_for_approvals(updates: &std::sync::Arc<std::sync::Mutex<Vec<AcpUpdate>>>, count: usize) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        let seen = updates
            .lock()
            .unwrap()
            .iter()
            .filter(|update| {
                matches!(
                    update,
                    AcpUpdate::Event(TaskEvent::ApprovalRequested { .. })
                )
            })
            .count();
        if seen >= count {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("approval {count} was not displayed in time");
}
