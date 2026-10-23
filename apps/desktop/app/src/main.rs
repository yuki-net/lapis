mod connection;
mod workspace_dialog;

use std::sync::Arc;
use std::time::Duration;

use lapis_desktop_client::{
    ConversationSession, EditorSession, GitSession, LspSession, SettingsSession, TaskSession,
    TerminalSession, WorkspaceSearchSession,
};
use lapis_host::BackendHost;
use lapis_persistence::{
    LocalConversationRepository, LocalGlobalSettingsRepository, LocalWorkspaceStateRepository,
};
use lapis_platform::{
    ConnectionGate, LocalLspBackend, LocalWorkspaceSearchBackend, LoopbackBackend, run_task_worker,
};

/// Dispatches worker and task smoke modes, or starts the desktop with command-line options.
fn main() {
    let arguments = std::env::args_os().collect::<Vec<_>>();
    if arguments
        .get(1)
        .is_some_and(|value| value == "--managed-host")
    {
        let result = arguments
            .get(2)
            .ok_or("Workspaceがありません")
            .map_err(|error| -> Box<dyn std::error::Error + Send + Sync> { error.into() })
            .and_then(|root| lapis_host::run_managed_host(std::path::Path::new(root)));
        if result.is_err() {
            std::process::exit(1);
        }
        return;
    }
    if arguments.get(1).map(std::ffi::OsString::as_os_str)
        == Some(std::ffi::OsStr::new("--task-worker"))
    {
        let Some(spec_path) = arguments.get(2) else {
            eprintln!("Task worker spec がありません");
            std::process::exit(2);
        };
        if let Err(error) = run_task_worker(std::path::Path::new(&spec_path)) {
            eprintln!("Task worker failed: {error}");
            std::process::exit(1);
        }
        return;
    }
    if arguments.get(1).map(std::ffi::OsString::as_os_str)
        == Some(std::ffi::OsStr::new("--task-smoke"))
    {
        run_task_smoke(&arguments);
        return;
    }
    let Some(startup) = connection::Startup::open(&arguments).unwrap_or_else(|error| {
        if arguments
            .iter()
            .any(|argument| argument == "--connect-host")
        {
            rfd::MessageDialog::new()
                .set_title("Hostに接続できません")
                .set_description(error.to_string())
                .set_level(rfd::MessageLevel::Error)
                .show();
        }
        eprintln!("Host接続に失敗しました: {error}");
        std::process::exit(1);
    }) else {
        return;
    };
    let is_remote = startup.root.is_some();
    let use_loopback = is_remote || arguments.iter().any(|value| value == "--loopback");
    let connection = if is_remote {
        ConnectionGate::default()
    } else {
        ConnectionGate::connected()
    };
    let host = startup.backend;
    let state = Arc::new(LocalWorkspaceStateRepository::new(
        startup.directory.join("session-v1.txt"),
    ));
    let mut session = if let Some(root) = startup.root.as_ref() {
        use lapis_desktop_client::WorkspaceStateRepository;
        let mut session = EditorSession::new_empty(
            host.workspaces(),
            Arc::new(workspace_dialog::NativeWorkspaceDialog),
            state.clone(),
            host.documents(),
        );
        let restored = state.load().unwrap_or_else(|error| {
            eprintln!("復元状態を読み込めません: {error}");
            std::process::exit(1);
        });
        if let Some(snapshot) = restored {
            session.restore_snapshot(snapshot).unwrap_or_else(|error| {
                eprintln!("Remoteの復元に失敗しました: {error}");
                std::process::exit(1);
            });
        } else {
            session
                .open_workspace(root.clone())
                .unwrap_or_else(|error| {
                    eprintln!("Remoteを開けません: {error}");
                    std::process::exit(1);
                });
        }
        session
    } else {
        EditorSession::new(
            host.workspaces(),
            Arc::new(workspace_dialog::NativeWorkspaceDialog),
            state,
            host.documents(),
        )
    };
    if let Some(index) = option_index(&arguments, "--workspace") {
        let Some(root) = arguments.get(index + 1).map(std::path::PathBuf::from) else {
            eprintln!("Usage: lapis --workspace <directory>");
            std::process::exit(2);
        };
        if let Err(error) = session.open_workspace(root) {
            eprintln!("Workspace open failed: {error}");
            std::process::exit(1);
        }
    }
    if let Some(index) = option_index(&arguments, "--open-file") {
        let Some(path) = arguments.get(index + 1).map(std::path::PathBuf::from) else {
            eprintln!("Usage: lapis --open-file <path>");
            std::process::exit(2);
        };
        let path = if path.is_absolute() {
            path
        } else {
            session
                .workspace_root()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join(path)
        };
        if let Err(error) = session.open_path(path) {
            eprintln!("File open failed: {error}");
            std::process::exit(1);
        }
    }
    let conversation_session = ConversationSession::new(
        Arc::new(LocalConversationRepository::new(
            startup.directory.join("conversations-v1.json"),
        )),
        session.snapshot(),
    );
    let settings_session = SettingsSession::load(Arc::new(LocalGlobalSettingsRepository::new(
        startup.directory.join("settings-v1.json"),
    )))
    .unwrap_or_else(|error| {
        eprintln!("Global settings load failed: {error}");
        SettingsSession::load(Arc::new(LocalGlobalSettingsRepository::new(
            std::env::temp_dir().join("lapis-settings-fallback.json"),
        )))
        .expect("temporary global settings must be available")
    });
    if let Err(error) = conversation_session.restore_matching_workspace(&mut session) {
        eprintln!("Conversation restore failed: {error}");
    }
    if let Some(root) = session.workspace_root() {
        host.register_workspace(root)
            .expect("Workspace registration must succeed");
    }
    let mut task_session = TaskSession::new(host.tasks());
    if let Some(workspace) = session.workspace_id()
        && let Err(error) = task_session.refresh(workspace)
    {
        eprintln!("Task restore failed: {error}");
    }
    let git_session = GitSession::new(host.git());
    let lsp_session = if use_loopback {
        LspSession::new(Arc::new(LoopbackBackend::new(
            Arc::new(LocalLspBackend::default()),
            connection.clone(),
        )))
    } else {
        LspSession::new(Arc::new(LocalLspBackend::default()))
    };
    let mut terminal_session = TerminalSession::new(host.terminals());
    let search_session = if use_loopback {
        WorkspaceSearchSession::new(Arc::new(LoopbackBackend::new(
            Arc::new(LocalWorkspaceSearchBackend),
            connection,
        )))
    } else {
        WorkspaceSearchSession::new(Arc::new(LocalWorkspaceSearchBackend))
    };

    let show_terminal = arguments.iter().any(|value| value == "--show-terminal");
    if show_terminal
        && let Some(root) = session.workspace_root()
        && let Err(error) = terminal_session.start(root, 120, 30)
    {
        eprintln!("Terminal start failed: {error}");
    }
    let initial_view = lapis_desktop_ui::InitialView {
        empty_window: false,
        native_window_smoke: arguments.iter().any(|value| value == "--window-host-smoke"),
        show_tasks: arguments.iter().any(|value| value == "--show-tasks"),
        show_terminal,
        show_problems: arguments.iter().any(|value| value == "--show-problems"),
    };
    let desktop_services = lapis_desktop_ui::DesktopServices::new(
        task_session,
        git_session,
        lsp_session,
        terminal_session,
        search_session,
        conversation_session,
        settings_session,
    )
    .with_host_connector(move || connection::open_picker(startup.connector_directory.clone()));
    lapis_desktop_ui::run(session, desktop_services, initial_view);
}

fn option_index(arguments: &[std::ffi::OsString], name: &str) -> Option<usize> {
    arguments.iter().rposition(|value| value == name)
}

fn run_task_smoke(arguments: &[std::ffi::OsString]) {
    let Some(workspace_root) = arguments.get(2).map(std::path::PathBuf::from) else {
        eprintln!("Usage: lapis --task-smoke <workspace> [prompt]");
        std::process::exit(2);
    };
    let prompt = arguments
        .get(3)
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "ファイルを変更せず、Lapis とだけ回答してください。".to_owned());
    let auto_interact = arguments.iter().any(|value| value == "--auto-interact");
    let cancel_on_running = arguments.iter().any(|value| value == "--cancel-on-running");
    let mode = if arguments.iter().any(|value| value == "--plan") {
        lapis_task::TaskMode::Plan
    } else {
        lapis_task::TaskMode::Default
    };
    let host = BackendHost::start().expect("Backend Host must start");
    let workspace = host
        .documents()
        .workspace(&workspace_root)
        .expect("Workspace must register");
    let mut session = TaskSession::new(host.tasks());
    let execution_id = match session.start_codex_with_mode(workspace.clone(), prompt, mode) {
        Ok(id) => id,
        Err(error) => {
            eprintln!("Task start failed: {error}");
            std::process::exit(1);
        }
    };
    println!("execution={execution_id}");
    let mut last_sequence = 0;
    let mut last_controlled_status = None;
    for _ in 0..900 {
        std::thread::sleep(Duration::from_millis(200));
        if let Err(error) = session.refresh(&workspace) {
            eprintln!("Task refresh failed: {error}");
            std::process::exit(1);
        }
        let Some(record) = session
            .records()
            .iter()
            .find(|record| record.execution.id == execution_id)
            .cloned()
        else {
            continue;
        };
        for event in &record.events {
            if event.sequence <= last_sequence {
                continue;
            }
            println!("{} {}", event.sequence, event.event.display_text());
            last_sequence = event.sequence;
        }
        let execution_status = record.execution.status;
        if (auto_interact || cancel_on_running) && last_controlled_status != Some(execution_status)
        {
            let control = match execution_status {
                lapis_task::ExecutionStatus::Running if cancel_on_running => {
                    Some(lapis_task::TaskControl::Cancel)
                }
                lapis_task::ExecutionStatus::WaitingForInput => {
                    auto_interact.then(|| lapis_task::TaskControl::Reply {
                        text: "青".to_owned(),
                    })
                }
                lapis_task::ExecutionStatus::WaitingForApproval => {
                    auto_interact.then_some(lapis_task::TaskControl::Decline)
                }
                _ => None,
            };
            if let Some(control) = control {
                if let Err(error) = session.control(&execution_id, control) {
                    eprintln!("Task control failed: {error}");
                    std::process::exit(1);
                }
                last_controlled_status = Some(execution_status);
            }
        }
        if record.execution.status.is_terminal() {
            println!("status={}", record.execution.status.label());
            let expected = if cancel_on_running {
                lapis_task::ExecutionStatus::Cancelled
            } else {
                lapis_task::ExecutionStatus::Succeeded
            };
            if record.execution.status != expected {
                std::process::exit(1);
            }
            return;
        }
    }
    eprintln!("Task smoke timed out");
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::option_index;
    use std::ffi::OsString;

    #[test]
    fn repeated_workspace_options_use_the_last_value() {
        let arguments = [
            "lapis",
            "--workspace",
            "default-workspace",
            "--workspace",
            "explicit-workspace",
        ]
        .map(OsString::from);
        assert_eq!(option_index(&arguments, "--workspace"), Some(3));
    }
}
